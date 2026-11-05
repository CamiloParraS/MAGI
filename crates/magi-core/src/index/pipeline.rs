//! hash -> extract -> chunk -> embed -> write. The stages live in
//! [`extract`] and [`embed`]; this module holds their types and the one-shot
//! [`index_root`] that runs them on the calling thread.

mod embed;
mod extract;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use rusqlite::Connection;

use crate::db::files::{self, FileState, StoredFile};
use crate::db::roots;
use crate::discovery::{self, Kind, WalkEntry, WalkOptions};
use crate::dto::FileErrorCode;
use crate::embed::{ImageEmbedder, TextEmbedder};
use crate::error::{Error, Result};
use crate::extract::RawChunk;
use crate::ocr::OcrEngine;
use crate::platform::{FsProbe, PermissionProbe, RootAccess};
use crate::watch::reconcile::{next_scan_id, reconcile_root, remove_unseen};

pub(crate) use embed::{EMBED_BATCH, embed, embed_group, store_embedded};
use extract::FileOutcome;
pub(crate) use extract::{hash_file, prepare};

use super::gate::ImageGate;
use super::lifecycle::{self, FileStep};
use super::scheduler::{self, Action};
use super::{ModelIds, requeue_on_model_change};

/// What an indexing run needs beyond the walk options: the models.
pub struct IndexContext {
    /// `Arc` because the runtime shares the models between threads. `None`
    /// when search by meaning is not running.
    pub embedder: Option<Arc<dyn TextEmbedder>>,
    /// `Arc` because extraction runs on a detached thread that must own it.
    /// `None` when reading text in images is not running.
    pub ocr: Option<Arc<dyn OcrEngine>>,
    /// `None` leaves `vec_image` empty: images are still found by their text.
    pub image_embedder: Option<Arc<dyn ImageEmbedder>>,
    /// Bounds concurrent image decodes.
    pub image_gate: Arc<ImageGate>,
}

impl Default for IndexContext {
    /// No feature running: keyword and filename indexing only.
    fn default() -> Self {
        Self {
            embedder: None,
            ocr: None,
            image_embedder: None,
            image_gate: Arc::default(),
        }
    }
}

impl IndexContext {
    /// Meaning on, OCR and visual off; images still get QR payloads and a thumbnail.
    pub fn new(embedder: Arc<dyn TextEmbedder>) -> Self {
        Self {
            embedder: Some(embedder),
            ..Self::default()
        }
    }
}

pub struct IndexRootOptions {
    pub exclude_globs: Vec<glob::Pattern>,
    pub include_hidden: bool,
    pub follow_symlinks: bool,
    pub max_file_size_mb: u64,
    pub max_image_megapixels: u32,
    /// Kinds that get content extraction; others are indexed by filename only.
    pub file_types: Vec<Kind>,
    /// Lets a running scan stop early; shared by every options value the
    /// engine swaps in.
    pub scan: std::sync::Arc<crate::watch::reconcile::ScanControl>,
}

impl IndexRootOptions {
    /// Compiles the config's exclude globs, reusing the same
    /// `glob::Pattern` validation as `Config::validate`.
    pub fn from_config(config: &crate::config::IndexingConfig) -> Result<Self> {
        let exclude_globs = config
            .exclude_globs
            .iter()
            .map(|glob| {
                glob::Pattern::new(glob).map_err(|e| Error::InvalidGlob {
                    glob: glob.clone(),
                    reason: e.to_string(),
                })
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(Self {
            exclude_globs,
            include_hidden: config.include_hidden,
            follow_symlinks: config.follow_symlinks,
            max_file_size_mb: config.max_file_size_mb,
            max_image_megapixels: config.max_image_megapixels,
            file_types: config.file_types.clone(),
            scan: Default::default(),
        })
    }
}

impl IndexRootOptions {
    pub(crate) fn walk_options(&self) -> WalkOptions {
        WalkOptions {
            exclude_globs: self.exclude_globs.clone(),
            include_hidden: self.include_hidden,
            follow_symlinks: self.follow_symlinks,
        }
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct IndexSummary {
    pub indexed: u32,
    pub skipped: u32,
    pub errored: u32,
    /// Files whose indexed content was still right, so nothing was
    /// re-extracted or re-embedded.
    pub unchanged: u32,
    /// Files that were renamed or moved: kept as they were, not re-embedded.
    pub moved: u32,
    /// Files that are gone from disk (or no longer wanted) and were removed.
    pub removed: u32,
}

/// Brings the database in line with the files under `root_path`: reconciles
/// the walk against the rows (new and changed files become `pending`, vanished
/// ones are deleted, moved ones are renamed in place), then works the pending
/// queue. A file whose indexed content is still right is left alone; anything
/// else is extracted, chunked, embedded and written. Files marked stale by a
/// model, engine or pipeline-version change are re-done even though their
/// content is identical ([`requeue_on_model_change`]).
///
/// Each run is a new scan ([`next_scan_id`]): rows it does not see are the
/// deletions. A root that is missing or unreadable keeps its rows and its
/// status says why (SPEC.md §5.4).
///
/// One-shot and synchronous: the engine runs the same stages on its own threads.
pub fn index_root(
    conn: &mut Connection,
    root_id: i64,
    root_path: &Path,
    options: &IndexRootOptions,
    ctx: &IndexContext,
) -> Result<IndexSummary> {
    files::reset_indexing_to_pending(conn)?;
    requeue_on_model_change(
        conn,
        &ModelIds {
            text: ctx.embedder.as_deref().map(|e| e.model_id()),
            image: ctx.image_embedder.as_deref().map(|e| e.model_id()),
            ocr: ctx.ocr.as_deref().map(|o| o.engine_id()),
        },
    )?;

    let access = FsProbe.probe(root_path);
    roots::set_access(conn, root_id, &access)?;
    if access != RootAccess::Ok {
        return Ok(IndexSummary::default());
    }

    let scan_id = next_scan_id(conn)?;
    let (found, unseen) = reconcile_root(conn, root_id, root_path, options, scan_id)?;
    let mut summary = IndexSummary {
        unchanged: found.unchanged,
        moved: found.moved,
        removed: remove_unseen(conn, unseen)?,
        ..IndexSummary::default()
    };
    drain_pending(conn, ctx, options, &mut summary)?;
    Ok(summary)
}

/// Works the `pending` queue, newest first, until nothing is ready, making the
/// same state changes as the engine ([`lifecycle::apply_alone`]). A file that has vanished
/// since the scan is deleted instead; one that cannot be stat'd is put back
/// with a delay.
fn drain_pending(
    conn: &mut Connection,
    ctx: &IndexContext,
    options: &IndexRootOptions,
    summary: &mut IndexSummary,
) -> Result<()> {
    let root_paths: HashMap<i64, PathBuf> = roots::list(conn)?
        .into_iter()
        .map(|r| (r.id, r.path))
        .collect();
    loop {
        let now = unix_now();
        let batch = files::next_pending(conn, now, 256)?;
        if batch.is_empty() {
            return Ok(());
        }
        for pending in batch {
            // No stability wait: a one-shot run takes the files as they are.
            let action = match discovery::stat(&pending.path) {
                Ok(entry) => Action::Extract {
                    file: pending,
                    entry,
                },
                Err(e) => scheduler::gone_or_failed(pending.id, &e),
            };
            let (step, job) = lifecycle::begin(conn, action, &root_paths);
            count(summary, lifecycle::apply_alone(conn, step)?);
            if let Some(job) = job {
                let step = index_file(ctx, options, job)?;
                count(summary, lifecycle::apply_alone(conn, step)?);
            }
        }
    }
}

fn count(summary: &mut IndexSummary, status: Option<Status>) {
    if let Some(status) = status {
        match status {
            Status::Indexed => summary.indexed += 1,
            Status::Skipped => summary.skipped += 1,
            Status::Errored => summary.errored += 1,
            Status::Unchanged => summary.unchanged += 1,
            Status::Retried => {}
            Status::Removed => summary.removed += 1,
        }
    }
}

pub(crate) fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

/// One file to bring up to date, where it was found, and what the index knows
/// of it. Owned, so it can cross threads.
#[derive(Clone)]
pub(crate) struct Job {
    pub root_id: i64,
    pub root_path: PathBuf,
    pub entry: WalkEntry,
    /// The file's row: every queued file has one.
    pub stored: StoredFile,
}

pub(crate) enum Status {
    Indexed,
    Skipped,
    Errored,
    Unchanged,
    /// Failed for a reason that may pass; back in the queue with a delay.
    Retried,
    /// Gone from disk: its row, chunks and vectors were deleted.
    Removed,
}

/// What the extract stage decided. It touches no database: the writer applies
/// the result.
pub(crate) enum Extracted {
    /// Nothing to redo: only size, mtime, scan and state are refreshed.
    Keep { state: FileState },
    /// Read and extracted, ready to embed.
    Fresh(Box<Fresh>),
    /// Could not be read just now (I/O): try again later, with backoff.
    Retry {
        code: FileErrorCode,
        message: String,
    },
}

pub(crate) struct Fresh {
    outcome: FileOutcome,
    /// Content chunks, filename chunk last.
    chunks: Vec<RawChunk>,
    thumb_key: Option<String>,
    /// `Feature::bit`s of the search features this file is indexed without.
    features_missing: i64,
}

impl Fresh {
    pub(crate) fn chunks_len(&self) -> usize {
        self.chunks.len()
    }
}

pub(crate) struct Embedded {
    fresh: Fresh,
    /// `None` when search by meaning is not running.
    embeddings: Option<Vec<Vec<f32>>>,
}

/// Extracts and embeds one file on the calling thread; the step stores it.
fn index_file(ctx: &IndexContext, options: &IndexRootOptions, job: Job) -> Result<FileStep> {
    Ok(match prepare(ctx, options, &job) {
        Extracted::Keep { state } => FileStep::Keep {
            job: Box::new(job),
            state,
        },
        Extracted::Retry { code, message } => FileStep::Retry {
            file_id: job.stored.id,
            code,
            message,
        },
        Extracted::Fresh(fresh) => {
            let embedded = embed(ctx, &job, *fresh, &|| {})?;
            FileStep::Store {
                job: Box::new(job),
                embedded: Box::new(embedded),
            }
        }
    })
}

#[cfg(test)]
mod tests;
