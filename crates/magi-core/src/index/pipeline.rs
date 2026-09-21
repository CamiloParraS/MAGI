//! hash -> extract -> chunk -> embed -> write. Implemented starting M2

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use rusqlite::Connection;

use crate::db::files::{self, FileRecord, StoredFile, upsert_file};
use crate::db::roots;
use crate::discovery::{self, Kind, WalkEntry, WalkOptions};
use crate::embed::{ImageEmbedder, TextEmbedder};
use crate::error::{Error, Result};
use crate::extract::code::CodeExtractor;
use crate::extract::filename::filename_chunk;
use crate::extract::office::OfficeExtractor;
use crate::extract::pdf::PdfExtractor;
use crate::extract::text::TextExtractor;
use crate::extract::{ExtractedDoc, Extractor, RawChunk};
use crate::ocr::{NoOcr, OcrEngine};
use crate::platform::{FsProbe, PermissionProbe, RootAccess};
use crate::watch::reconcile::{reconcile_root, resolve_moves};

use super::{ModelIds, PIPELINE_VERSION, requeue_on_model_change};

/// What an indexing run needs beyond the walk options: the models.
pub struct IndexContext<'a> {
    pub embedder: &'a dyn TextEmbedder,
    /// `Arc` because extraction runs on a detached thread that must own it.
    pub ocr: Arc<dyn OcrEngine>,
    /// `None` leaves `vec_image` empty: images are still found by their text.
    pub image_embedder: Option<Arc<dyn ImageEmbedder>>,
}

impl<'a> IndexContext<'a> {
    /// A context with no OCR engine and no image embedder; images still get
    /// QR payloads and a thumbnail.
    pub fn new(embedder: &'a dyn TextEmbedder) -> Self {
        Self {
            embedder,
            ocr: Arc::new(NoOcr),
            image_embedder: None,
        }
    }
}

const SNIFF_HEADER_LEN: usize = 8192;

pub struct IndexRootOptions {
    pub exclude_globs: Vec<glob::Pattern>,
    pub include_hidden: bool,
    pub follow_symlinks: bool,
    pub max_file_size_mb: u64,
    pub max_image_megapixels: u32,
    /// Kinds that get content extraction; others are indexed by filename only.
    pub file_types: Vec<Kind>,
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
/// `scan_id` must be larger than on the previous run
/// ([`crate::watch::reconcile::next_scan_id`]): rows not seen by this scan are
/// the deletions. A root that is missing or unreadable keeps its rows and its
/// status says why (SPEC.md §5.4).
///
/// One-shot and synchronous: no watcher or worker threads (M5, later slices).
pub fn index_root(
    conn: &mut Connection,
    root_id: i64,
    root_path: &Path,
    options: &IndexRootOptions,
    scan_id: i64,
    ctx: &IndexContext,
) -> Result<IndexSummary> {
    files::reset_indexing_to_pending(conn)?;
    requeue_on_model_change(
        conn,
        &ModelIds {
            text: ctx.embedder.model_id(),
            image: ctx.image_embedder.as_deref().map(|e| e.model_id()),
            ocr: ctx.ocr.engine_id(),
        },
    )?;

    let access = FsProbe.probe(root_path);
    roots::set_access(conn, root_id, &access)?;
    if access != RootAccess::Ok {
        return Ok(IndexSummary::default());
    }

    let report = reconcile_root(conn, root_id, root_path, options, scan_id)?;
    let moves = resolve_moves(conn, report.unseen, scan_id)?;
    let mut summary = IndexSummary {
        unchanged: report.unchanged,
        moved: moves.moved,
        removed: moves.removed,
        ..IndexSummary::default()
    };
    drain_pending(conn, ctx, options, scan_id, &mut summary)?;
    Ok(summary)
}

/// Works the `pending` queue, newest first, until nothing is ready. A file
/// that has vanished since the scan is deleted instead; one that cannot be
/// stat'd is put back with a delay ([`files::record_failure`]).
fn drain_pending(
    conn: &mut Connection,
    ctx: &IndexContext,
    options: &IndexRootOptions,
    scan_id: i64,
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
            let entry = match discovery::stat(&pending.path) {
                Ok(entry) => entry,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    files::delete_file(conn, pending.id)?;
                    summary.removed += 1;
                    continue;
                }
                Err(e) => {
                    files::record_failure(conn, pending.id, &e.to_string(), now)?;
                    continue;
                }
            };
            let Some(root_path) = root_paths.get(&pending.root_id) else {
                continue;
            };
            let target = Target {
                root_id: pending.root_id,
                root_path,
                entry: &entry,
                scan_id,
            };
            match index_file(conn, ctx, options, target)? {
                Status::Indexed => summary.indexed += 1,
                Status::Skipped => summary.skipped += 1,
                Status::Errored => summary.errored += 1,
                Status::Unchanged => summary.unchanged += 1,
            }
        }
    }
}

pub(crate) fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

/// One file to bring up to date, and where it was found.
#[derive(Clone, Copy)]
struct Target<'a> {
    root_id: i64,
    root_path: &'a Path,
    entry: &'a WalkEntry,
    scan_id: i64,
}

enum Status {
    Indexed,
    Skipped,
    Errored,
    Unchanged,
}

/// What the file's stored row says about whether it can be kept as is.
fn is_settled(stored: &StoredFile) -> bool {
    stored.pipeline_version == PIPELINE_VERSION
        && matches!(stored.state.as_str(), "indexed" | "skipped" | "error")
}

/// A file whose kind is not extracted (unsupported, disabled, too large) has no
/// content hash; it is unchanged if it would be indexed the same way again.
fn unchanged_without_content(stored: &StoredFile, outcome: &FileOutcome) -> bool {
    stored.pipeline_version == PIPELINE_VERSION
        && stored.state != "error"
        && outcome.state != "error"
        && stored.kind == outcome.kind.as_str()
        && stored.content_hash.is_none()
        && stored.skip_reason.as_deref() == outcome.skip_reason
}

/// Same bytes as when it was indexed, by the current pipeline, with real
/// chunks (a row that was `skipped` or `error` is re-tried instead).
fn content_is(stored: &StoredFile, hash: &[u8; 32]) -> bool {
    stored.pipeline_version == PIPELINE_VERSION
        && stored.skip_reason.is_none()
        && matches!(stored.state.as_str(), "indexed" | "pending")
        && stored.content_hash.as_deref() == Some(hash.as_slice())
}

/// The stages for one file, in order: is it unchanged? (size and mtime, then
/// content hash) -> extract -> embed -> write in one transaction. The pieces
/// are separate so the runtime can run them on different threads.
fn index_file(
    conn: &mut Connection,
    ctx: &IndexContext,
    options: &IndexRootOptions,
    target: Target,
) -> Result<Status> {
    let Target {
        root_id,
        root_path,
        entry,
        scan_id,
    } = target;
    let stored = files::get_stored(conn, &entry.path)?;
    let keep = |conn: &Connection, id: i64, state: &str| {
        files::touch_unchanged(conn, id, entry.size, entry.mtime_ns, scan_id, state)
    };

    // Same size and mtime as when it was indexed: nothing to read.
    if let Some(s) = &stored
        && is_settled(s)
        && s.root_id == root_id
        && s.size == entry.size
        && s.mtime_ns == entry.mtime_ns
    {
        keep(conn, s.id, &s.state)?;
        return Ok(Status::Unchanged);
    }

    let max_size_bytes = options.max_file_size_mb.saturating_mul(1024 * 1024);
    let mut outcome = match plan_entry(&entry.path, entry.size, max_size_bytes, options) {
        Plan::Done(outcome) => {
            let outcome = *outcome;
            if let Some(s) = stored
                .as_ref()
                .filter(|s| unchanged_without_content(s, &outcome))
            {
                keep(conn, s.id, outcome.state)?;
                return Ok(Status::Unchanged);
            }
            outcome
        }
        Plan::Extract(kind) => {
            // Only a file already in the index has a hash to compare with; a
            // new one goes straight to extraction, which hashes it anyway.
            match stored.as_ref().map(|s| (s, hash_file(&entry.path))) {
                // Touched or re-saved with the same content: keep the chunks
                // and vectors, update only size and mtime (SPEC.md §5.4 step 4).
                Some((s, Ok(hash))) if content_is(s, &hash) => {
                    keep(conn, s.id, "indexed")?;
                    return Ok(Status::Unchanged);
                }
                Some((_, Err(e))) => errored(kind, e.to_string()),
                _ => extract_entry(&entry.path, kind, options, ctx),
            }
        }
    };

    let rel_path = entry.path.strip_prefix(root_path).unwrap_or(&entry.path);
    let file_name = entry
        .path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default();
    let ext = entry.path.extension().and_then(|e| e.to_str());

    let thumb_key = store_thumbnail(&entry.path, &outcome);
    let mut chunks = std::mem::take(&mut outcome.doc.chunks);
    chunks.push(filename_chunk(rel_path));
    let embeddings = embed_chunks(ctx.embedder, &entry.path, &mut chunks, &mut outcome)?;

    let record = FileRecord {
        root_id,
        path: &entry.path,
        rel_path,
        file_name,
        ext,
        kind: outcome.kind.as_str(),
        size: entry.size,
        mtime_ns: entry.mtime_ns,
        lang: outcome.doc.lang.as_deref(),
        state: outcome.state,
        skip_reason: outcome.skip_reason,
        error: outcome.error.as_deref(),
        seen_scan_id: scan_id,
        content_hash: outcome.content_hash.as_ref().map(|h| h.as_slice()),
        thumb_key: thumb_key.as_deref(),
    };
    upsert_file(
        conn,
        &record,
        &chunks,
        &embeddings,
        outcome.doc.image_embedding.as_deref(),
    )?;

    Ok(match outcome.state {
        "indexed" => Status::Indexed,
        "skipped" => Status::Skipped,
        _ => Status::Errored,
    })
}

/// Embeds `chunks` (content chunks, filename chunk last). One bad file must
/// not abort the run: if embedding fails, the file keeps only its filename
/// chunk (still searchable by name) and is marked `error`. If even that
/// fails, the embedder itself is broken and the error propagates rather than
/// erroring every remaining file.
fn embed_chunks(
    embedder: &dyn TextEmbedder,
    path: &Path,
    chunks: &mut Vec<RawChunk>,
    outcome: &mut FileOutcome,
) -> Result<Vec<Vec<f32>>> {
    let texts: Vec<&str> = chunks.iter().map(|c| c.text.as_str()).collect();
    match embedder.embed_passages(&texts) {
        Ok(embeddings) => Ok(embeddings),
        Err(e) => {
            tracing::warn!(path = %path.display(), error = %e, "embedding failed");
            outcome.state = "error";
            outcome.error = Some(e.to_string());
            chunks.drain(..chunks.len() - 1);
            embedder.embed_passages(&[chunks[0].text.as_str()])
        }
    }
}

/// A failed write costs the file its thumbnail, never its index entry.
fn store_thumbnail(path: &Path, outcome: &FileOutcome) -> Option<String> {
    let (hash, image) = outcome
        .content_hash
        .as_ref()
        .zip(outcome.doc.thumbnail.as_ref())?;
    crate::thumbs::store(hash, image)
        .inspect_err(
            |e| tracing::warn!(path = %path.display(), error = %e, "thumbnail not written"),
        )
        .ok()
}

struct FileOutcome {
    kind: Kind,
    state: &'static str,
    skip_reason: Option<&'static str>,
    error: Option<String>,
    content_hash: Option<[u8; 32]>,
    doc: ExtractedDoc,
}

fn indexed_no_chunks(kind: Kind) -> FileOutcome {
    FileOutcome {
        kind,
        state: "indexed",
        skip_reason: None,
        error: None,
        content_hash: None,
        doc: ExtractedDoc::default(),
    }
}

/// Not broken, just outside what is indexed: SPEC.md section 5.4's `skipped`.
fn skipped(kind: Kind, reason: &'static str) -> FileOutcome {
    FileOutcome {
        state: "skipped",
        skip_reason: Some(reason),
        ..indexed_no_chunks(kind)
    }
}

fn errored(kind: Kind, message: String) -> FileOutcome {
    FileOutcome {
        state: "error",
        error: Some(message),
        ..indexed_no_chunks(kind)
    }
}

fn read_header(path: &Path, len: usize) -> std::io::Result<Vec<u8>> {
    use std::io::Read;
    let mut file = std::fs::File::open(path)?;
    let mut buf = vec![0u8; len];
    let n = file.read(&mut buf)?;
    buf.truncate(n);
    Ok(buf)
}

/// What to do with a file before reading its content.
enum Plan {
    /// Settled without reading it: too large, unsupported, or unreadable.
    Done(Box<FileOutcome>),
    /// Read it and run this kind's extractor.
    Extract(Kind),
}

fn plan_entry(path: &Path, size: u64, max_size_bytes: u64, options: &IndexRootOptions) -> Plan {
    if size > max_size_bytes {
        return Plan::Done(Box::new(skipped(
            discovery::classify(path, &[]),
            "too_large",
        )));
    }

    // Classification from the extension alone needs no I/O; only sniff a
    // small header when the extension doesn't resolve a kind.
    let mut kind = discovery::classify(path, &[]);
    if kind == Kind::Other {
        match read_header(path, SNIFF_HEADER_LEN) {
            Ok(header) => kind = discovery::classify(path, &header),
            Err(e) => return Plan::Done(Box::new(errored(Kind::Other, e.to_string()))),
        }
    }

    // `Other` has no extractor, and a kind the user disabled in `file_types`
    // is indexed by filename only: either way the bytes would be discarded,
    // so don't read them.
    if kind == Kind::Other || !options.file_types.contains(&kind) {
        return Plan::Done(Box::new(indexed_no_chunks(kind)));
    }
    Plan::Extract(kind)
}

/// blake3 of the file, streamed so a touched large file is not loaded whole
/// just to learn it did not change.
pub(crate) fn hash_file(path: &Path) -> std::io::Result<[u8; 32]> {
    let mut hasher = blake3::Hasher::new();
    hasher.update_reader(std::fs::File::open(path)?)?;
    Ok(*hasher.finalize().as_bytes())
}

fn extract_entry(
    path: &Path,
    kind: Kind,
    options: &IndexRootOptions,
    ctx: &IndexContext,
) -> FileOutcome {
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) => return errored(kind, e.to_string()),
    };
    // Hashed from the bytes about to be extracted, not carried over from the
    // skip check: the file may have changed in between.
    let content_hash = Some(*blake3::hash(&bytes).as_bytes());

    let (path_buf, ocr, image_embedder, max_megapixels) = (
        path.to_path_buf(),
        Arc::clone(&ctx.ocr),
        ctx.image_embedder.clone(),
        options.max_image_megapixels,
    );
    let extracted = super::isolate::run(path.to_path_buf(), move || {
        let (path, bytes) = (path_buf.as_path(), bytes.as_slice());
        match kind {
            Kind::Text => TextExtractor.extract(path, bytes),
            Kind::Code => CodeExtractor.extract(path, bytes),
            Kind::Office => OfficeExtractor.extract(path, bytes),
            Kind::Pdf => {
                let mut doc = PdfExtractor.extract(path, bytes)?;
                // A PDF we can read but not render is still searchable.
                doc.thumbnail = crate::extract::pdf::first_page_thumbnail(bytes)
                    .inspect_err(
                        |e| tracing::warn!(path = %path.display(), error = %e, "no PDF thumbnail"),
                    )
                    .ok();
                Ok(doc)
            }
            Kind::Image => crate::extract::image::extract_image(
                path,
                bytes,
                max_megapixels,
                &*ocr,
                image_embedder.as_deref(),
            ),
            Kind::Other => Ok(ExtractedDoc::default()),
        }
    });

    match extracted {
        Ok(doc) => FileOutcome {
            content_hash,
            doc,
            ..indexed_no_chunks(kind)
        },
        // SPEC.md section 5.2: images over `max_image_megapixels` are
        // skipped. A real 75 MP panorama is not an error to retry.
        Err(Error::ImageTooLarge { .. }) => FileOutcome {
            content_hash,
            ..skipped(kind, "image_too_large")
        },
        Err(e) => FileOutcome {
            content_hash,
            ..errored(kind, e.to_string())
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;
    use crate::embed::{FakeEmbedder, TextEmbedder};
    use std::fs;

    /// Returns `(db_dir, root_dir, conn, root_id)`. Both temp dirs must
    /// stay in scope for the duration of the test, or they're deleted.
    fn open_test_db() -> (tempfile::TempDir, tempfile::TempDir, Connection, i64) {
        let db_dir = tempfile::tempdir().unwrap();
        let conn = db::open(&db_dir.path().join("magi.db")).unwrap();
        let root_dir = tempfile::tempdir().unwrap();
        let root = db::roots::add(&conn, root_dir.path()).unwrap();
        (db_dir, root_dir, conn, root.id)
    }

    /// `index_root` with the defaults every test but two uses.
    fn index_fake(conn: &mut Connection, root_id: i64, root_path: &Path) -> IndexSummary {
        index_root(
            conn,
            root_id,
            root_path,
            &default_options(),
            1,
            &IndexContext::new(&FakeEmbedder),
        )
        .unwrap()
    }

    fn default_options() -> IndexRootOptions {
        IndexRootOptions {
            exclude_globs: vec![glob::Pattern::new("**/node_modules/**").unwrap()],
            include_hidden: false,
            follow_symlinks: false,
            max_file_size_mb: 1,
            max_image_megapixels: 64,
            file_types: vec![Kind::Text, Kind::Code, Kind::Pdf, Kind::Office, Kind::Image],
        }
    }

    #[test]
    fn indexes_text_files_and_makes_them_searchable() {
        let (_db_dir, root_dir, mut conn, root_id) = open_test_db();
        let root_path = crate::paths::canonicalize(root_dir.path()).unwrap();
        fs::write(
            root_path.join("english.txt"),
            "The quick brown fox jumps over the lazy dog.",
        )
        .unwrap();
        fs::write(
            root_path.join("spanish.txt"),
            "El veloz murciélago hindú comía feliz cardillo y kiwi.",
        )
        .unwrap();

        let summary = index_fake(&mut conn, root_id, &root_path);

        assert_eq!(summary.indexed, 2);
        assert_eq!(summary.skipped, 0);
        assert_eq!(summary.errored, 0);
        assert_eq!(db::files::count_files(&conn).unwrap(), 2);

        let hits = crate::search::fts::search_fts(&conn, "murciélago", 10).unwrap();
        assert_eq!(hits.len(), 1);
    }

    #[test]
    fn image_over_the_megapixel_cap_is_skipped_not_errored() {
        let (_db_dir, root_dir, mut conn, root_id) = open_test_db();
        let root_path = crate::paths::canonicalize(root_dir.path()).unwrap();
        // Declares 20000 x 20000 (400 MP) in 1.1 MB, so it is over the default
        // 64 MP cap and refused from its header.
        let bomb = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/corpus/edge/bomb.png");
        fs::copy(bomb, root_path.join("panorama.png")).unwrap();

        // The test default caps files at 1 MB and the bomb is 1.1 MB: raise it
        // so the image, not the file-size check, is what refuses it.
        let options = IndexRootOptions {
            max_file_size_mb: 10,
            ..default_options()
        };
        let summary = index_root(
            &mut conn,
            root_id,
            &root_path,
            &options,
            1,
            &IndexContext::new(&FakeEmbedder),
        )
        .unwrap();

        assert_eq!((summary.skipped, summary.errored), (1, 0));
        let row = db::files::get_by_path(&conn, &root_path.join("panorama.png"))
            .unwrap()
            .unwrap();
        assert_eq!(row.state, "skipped");
        assert_eq!(row.skip_reason.as_deref(), Some("image_too_large"));
    }

    #[test]
    fn oversized_file_is_skipped_with_reason() {
        let (_db_dir, root_dir, mut conn, root_id) = open_test_db();
        let root_path = crate::paths::canonicalize(root_dir.path()).unwrap();
        let huge = "a".repeat(2 * 1024 * 1024); // 2 MB > 1 MB cap
        fs::write(root_path.join("huge.txt"), huge).unwrap();

        let summary = index_fake(&mut conn, root_id, &root_path);

        assert_eq!(summary.skipped, 1);
        let row = db::files::get_by_path(&conn, &root_path.join("huge.txt"))
            .unwrap()
            .unwrap();
        assert_eq!(row.state, "skipped");
        assert_eq!(row.skip_reason.as_deref(), Some("too_large"));
    }

    #[test]
    fn empty_file_is_indexed_with_only_a_filename_chunk() {
        let (_db_dir, root_dir, mut conn, root_id) = open_test_db();
        let root_path = crate::paths::canonicalize(root_dir.path()).unwrap();
        fs::write(root_path.join("empty.txt"), "").unwrap();

        let summary = index_fake(&mut conn, root_id, &root_path);

        assert_eq!(summary.indexed, 1);
        let hits = crate::search::fts::search_fts(&conn, "empty", 10).unwrap();
        assert_eq!(hits.len(), 1);
    }

    #[test]
    fn unsupported_file_type_is_indexed_by_filename_only() {
        let (_db_dir, root_dir, mut conn, root_id) = open_test_db();
        let root_path = crate::paths::canonicalize(root_dir.path()).unwrap();
        fs::write(root_path.join("archive.zip"), b"PK\x03\x04").unwrap();

        let summary = index_fake(&mut conn, root_id, &root_path);

        assert_eq!(summary.indexed, 1);
        let row = db::files::get_by_path(&conn, &root_path.join("archive.zip"))
            .unwrap()
            .unwrap();
        assert_eq!(row.state, "indexed");
        assert_eq!(row.kind, "other");
        let hits = crate::search::fts::search_fts(&conn, "archive", 10).unwrap();
        assert_eq!(hits.len(), 1);
    }

    #[test]
    fn excluded_directory_is_never_indexed() {
        let (_db_dir, root_dir, mut conn, root_id) = open_test_db();
        let root_path = crate::paths::canonicalize(root_dir.path()).unwrap();
        fs::create_dir(root_path.join("node_modules")).unwrap();
        fs::write(root_path.join("node_modules/pkg.js"), "ignored").unwrap();
        fs::write(root_path.join("keep.txt"), "keep me").unwrap();

        let summary = index_fake(&mut conn, root_id, &root_path);

        assert_eq!(summary.indexed, 1);
        assert_eq!(db::files::count_files(&conn).unwrap(), 1);
    }

    #[test]
    fn reindexing_is_idempotent() {
        let (_db_dir, root_dir, mut conn, root_id) = open_test_db();
        let root_path = crate::paths::canonicalize(root_dir.path()).unwrap();
        fs::write(root_path.join("a.txt"), "alpha content").unwrap();
        fs::write(root_path.join("b.txt"), "beta content").unwrap();

        index_fake(&mut conn, root_id, &root_path);
        let first_count = db::files::count_files(&conn).unwrap();
        index_root(
            &mut conn,
            root_id,
            &root_path,
            &default_options(),
            2,
            &IndexContext::new(&FakeEmbedder),
        )
        .unwrap();
        let second_count = db::files::count_files(&conn).unwrap();

        assert_eq!(first_count, second_count);
        assert_eq!(second_count, 2);
    }

    #[test]
    fn pdf_files_are_extracted_per_page_and_searchable() {
        let (_db_dir, root_dir, mut conn, root_id) = open_test_db();
        let root_path = crate::paths::canonicalize(root_dir.path()).unwrap();
        let fixture =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/corpus/pdf/report.pdf");
        fs::copy(&fixture, root_path.join("report.pdf")).unwrap();

        let summary = index_fake(&mut conn, root_id, &root_path);

        let row = db::files::get_by_path(&conn, &root_path.join("report.pdf"))
            .unwrap()
            .unwrap();
        assert_eq!(summary.indexed, 1);
        assert_eq!(row.state, "indexed");
        assert_eq!(row.kind, "pdf");
        assert_eq!(row.lang.as_deref(), Some("en"));

        let hits = crate::search::fts::search_fts(&conn, "Expenses", 10).unwrap();
        assert_eq!(hits.len(), 1);
    }

    #[test]
    fn password_protected_pdf_is_indexed_by_filename_only_via_error_path() {
        let (_db_dir, root_dir, mut conn, root_id) = open_test_db();
        let root_path = crate::paths::canonicalize(root_dir.path()).unwrap();
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/corpus/edge/password_protected.pdf");
        fs::copy(&fixture, root_path.join("password_protected.pdf")).unwrap();

        let summary = index_fake(&mut conn, root_id, &root_path);

        assert_eq!(summary.errored, 1);
        let row = db::files::get_by_path(&conn, &root_path.join("password_protected.pdf"))
            .unwrap()
            .unwrap();
        assert_eq!(row.state, "error");
        // Still findable by name even though content extraction failed.
        let hits = crate::search::fts::search_fts(&conn, "protected", 10).unwrap();
        assert_eq!(hits.len(), 1);
    }

    #[test]
    fn office_files_are_extracted_and_searchable() {
        let (_db_dir, root_dir, mut conn, root_id) = open_test_db();
        let root_path = crate::paths::canonicalize(root_dir.path()).unwrap();
        let office_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/corpus/office");
        fs::copy(office_dir.join("notes.docx"), root_path.join("notes.docx")).unwrap();
        fs::copy(
            office_dir.join("kickoff.pptx"),
            root_path.join("kickoff.pptx"),
        )
        .unwrap();
        fs::copy(
            office_dir.join("inventory.xlsx"),
            root_path.join("inventory.xlsx"),
        )
        .unwrap();

        let summary = index_fake(&mut conn, root_id, &root_path);

        assert_eq!(summary.indexed, 3);
        for name in ["notes.docx", "kickoff.pptx", "inventory.xlsx"] {
            let row = db::files::get_by_path(&conn, &root_path.join(name))
                .unwrap()
                .unwrap();
            assert_eq!(row.state, "indexed");
            assert_eq!(row.kind, "office");
        }

        // Not "warehouse" alone: it appears in both notes.docx ("the new
        // warehouse") and inventory.xlsx ("Warehouse A"/"Warehouse B").
        assert_eq!(
            crate::search::fts::search_fts(&conn, "migration", 10)
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            crate::search::fts::search_fts(&conn, "presupuesto", 10)
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            crate::search::fts::search_fts(&conn, "widget", 10)
                .unwrap()
                .len(),
            1
        );
    }

    /// Real files from an actual nested user document tree (not
    /// synthetic tempfile writes), edge-case fixture
    /// list: a genuinely empty file, a file over the size cap, and a
    /// realistically deep directory — see `fixtures/corpus/edge/`.
    #[test]
    fn real_empty_file_is_indexed_with_only_a_filename_chunk() {
        let (_db_dir, root_dir, mut conn, root_id) = open_test_db();
        let root_path = crate::paths::canonicalize(root_dir.path()).unwrap();
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/corpus/edge/empty_real.gitignore");
        assert_eq!(fs::metadata(&fixture).unwrap().len(), 0);
        fs::copy(&fixture, root_path.join("empty_real.gitignore")).unwrap();

        let summary = index_fake(&mut conn, root_id, &root_path);

        assert_eq!(summary.indexed, 1);
        let row = db::files::get_by_path(&conn, &root_path.join("empty_real.gitignore"))
            .unwrap()
            .unwrap();
        assert_eq!(row.state, "indexed");
    }

    #[test]
    fn real_file_over_cap_is_skipped_with_reason() {
        let (_db_dir, root_dir, mut conn, root_id) = open_test_db();
        let root_path = crate::paths::canonicalize(root_dir.path()).unwrap();
        let fixture =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/corpus/edge/huge_real.pdf");
        assert!(fs::metadata(&fixture).unwrap().len() > 1024 * 1024);
        fs::copy(&fixture, root_path.join("huge_real.pdf")).unwrap();

        // default_options() caps at 1 MB; huge_real.pdf is ~1.4 MB.
        let summary = index_fake(&mut conn, root_id, &root_path);

        assert_eq!(summary.skipped, 1);
        let row = db::files::get_by_path(&conn, &root_path.join("huge_real.pdf"))
            .unwrap()
            .unwrap();
        assert_eq!(row.state, "skipped");
        assert_eq!(row.skip_reason.as_deref(), Some("too_large"));
    }

    #[test]
    fn real_deeply_nested_file_is_indexed_and_searchable() {
        let (_db_dir, root_dir, mut conn, root_id) = open_test_db();
        let root_path = crate::paths::canonicalize(root_dir.path()).unwrap();
        let fixture_root =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/corpus/edge/deep_real");
        let rel = Path::new(
            "Tercer Semestre/Fisica/asuntosSinImportancia/solarcalculator/solarcalculator/src/main/java/com/example/solarcalculator/model/calculateRequest.java",
        );
        // Rebuild component-by-component rather than a single `join(rel)`:
        // `rel`'s forward slashes survive verbatim inside a joined PathBuf
        // on Windows, producing mixed separators that don't string-match
        // the walker's own (all-backslash) path for the same file.
        let dest = rel
            .components()
            .fold(root_path.clone(), |acc, c| acc.join(c));
        fs::create_dir_all(dest.parent().unwrap()).unwrap();
        fs::copy(fixture_root.join(rel), &dest).unwrap();

        let summary = index_fake(&mut conn, root_id, &root_path);

        assert_eq!(summary.indexed, 1);
        let row = db::files::get_by_path(&conn, &dest).unwrap().unwrap();
        assert_eq!(row.state, "indexed");
        assert_eq!(row.kind, "code");
        assert_eq!(
            crate::search::fts::search_fts(&conn, "irradiacionSolar", 10)
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn code_files_are_chunked_by_symbol_and_searchable() {
        let (_db_dir, root_dir, mut conn, root_id) = open_test_db();
        let root_path = crate::paths::canonicalize(root_dir.path()).unwrap();
        let code_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/corpus/code");
        fs::copy(code_dir.join("sample.rs"), root_path.join("sample.rs")).unwrap();
        fs::copy(code_dir.join("muestra.py"), root_path.join("muestra.py")).unwrap();

        let summary = index_fake(&mut conn, root_id, &root_path);

        assert_eq!(summary.indexed, 2);
        for name in ["sample.rs", "muestra.py"] {
            let row = db::files::get_by_path(&conn, &root_path.join(name))
                .unwrap()
                .unwrap();
            assert_eq!(row.state, "indexed");
            assert_eq!(row.kind, "code");
        }

        assert_eq!(
            crate::search::fts::search_fts(&conn, "origin", 10)
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            crate::search::fts::search_fts(&conn, "calculadora", 10)
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn indexing_writes_vec_text_rows_and_records_model_id() {
        let (_db_dir, root_dir, mut conn, root_id) = open_test_db();
        let root_path = crate::paths::canonicalize(root_dir.path()).unwrap();
        fs::write(root_path.join("a.txt"), "hello there").unwrap();

        index_fake(&mut conn, root_id, &root_path);

        let chunk_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM chunks", [], |r| r.get(0))
            .unwrap();
        let vec_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM vec_text", [], |r| r.get(0))
            .unwrap();
        assert_eq!(vec_count, chunk_count);
        assert!(chunk_count > 0);

        assert_eq!(
            crate::db::meta::get(&conn, "text_model_id").unwrap(),
            Some(FakeEmbedder.model_id().to_string())
        );
    }

    /// A test-only `TextEmbedder` that delegates to `FakeEmbedder` for
    /// everything except `model_id`, so it can stand in for "a different
    /// embedding model" without a second real implementation.
    struct OtherFakeEmbedder;

    impl TextEmbedder for OtherFakeEmbedder {
        fn model_id(&self) -> &str {
            "other-fake-v1"
        }

        fn dim(&self) -> usize {
            FakeEmbedder.dim()
        }

        fn embed_passages(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
            FakeEmbedder.embed_passages(texts)
        }

        fn embed_query(&self, text: &str) -> Result<Vec<f32>> {
            FakeEmbedder.embed_query(text)
        }
    }

    /// Fails any batch containing a content chunk (more than one text);
    /// filename-only batches succeed.
    struct FlakyEmbedder;

    impl TextEmbedder for FlakyEmbedder {
        fn model_id(&self) -> &str {
            FakeEmbedder.model_id()
        }

        fn dim(&self) -> usize {
            FakeEmbedder.dim()
        }

        fn embed_passages(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
            if texts.len() > 1 {
                return Err(Error::Model("boom".into()));
            }
            FakeEmbedder.embed_passages(texts)
        }

        fn embed_query(&self, text: &str) -> Result<Vec<f32>> {
            FakeEmbedder.embed_query(text)
        }
    }

    #[test]
    fn disabled_file_type_is_indexed_by_filename_only() {
        let (_db_dir, root_dir, mut conn, root_id) = open_test_db();
        let root_path = crate::paths::canonicalize(root_dir.path()).unwrap();
        fs::write(root_path.join("a.txt"), "zebrafish").unwrap();

        let mut options = default_options();
        options.file_types.retain(|&k| k != Kind::Text);
        let summary = index_root(
            &mut conn,
            root_id,
            &root_path,
            &options,
            1,
            &IndexContext::new(&FakeEmbedder),
        )
        .unwrap();

        assert_eq!(summary.indexed, 1);
        let fts = |q| crate::search::fts::search_fts(&conn, q, 10).unwrap().len();
        assert_eq!(fts("zebrafish"), 0);
        assert_eq!(fts("a"), 1);
    }

    #[test]
    fn embedding_failure_marks_file_errored_and_run_continues() {
        let (_db_dir, root_dir, mut conn, root_id) = open_test_db();
        let root_path = crate::paths::canonicalize(root_dir.path()).unwrap();
        fs::write(root_path.join("a.txt"), "hello there").unwrap();
        fs::write(root_path.join("b.bin"), [0u8; 4]).unwrap(); // filename-only: embeds fine

        let summary = index_root(
            &mut conn,
            root_id,
            &root_path,
            &default_options(),
            1,
            &IndexContext::new(&FlakyEmbedder),
        )
        .unwrap();

        assert_eq!(summary.errored, 1);
        assert_eq!(summary.indexed, 1);
        let row = db::files::get_by_path(&conn, &root_path.join("a.txt"))
            .unwrap()
            .unwrap();
        assert_eq!(row.state, "error");
    }

    #[test]
    fn reindexing_with_a_different_embedder_overwrites_the_recorded_model_id() {
        let (_db_dir, root_dir, mut conn, root_id) = open_test_db();
        let root_path = crate::paths::canonicalize(root_dir.path()).unwrap();
        fs::write(root_path.join("a.txt"), "hello there").unwrap();

        index_fake(&mut conn, root_id, &root_path);
        assert_eq!(
            crate::db::meta::get(&conn, "text_model_id").unwrap(),
            Some(FakeEmbedder.model_id().to_string())
        );

        // Re-index with a different embedder: `previous_model_id` is read
        // (exercising that code path without panicking) and the stored id
        // still ends up overwritten with the new model's — this fix only
        // warns, it doesn't block the write. The re-embed *trigger* is M5's
        // job.
        index_root(
            &mut conn,
            root_id,
            &root_path,
            &default_options(),
            2,
            &IndexContext::new(&OtherFakeEmbedder),
        )
        .unwrap();
        assert_eq!(
            crate::db::meta::get(&conn, "text_model_id").unwrap(),
            Some(OtherFakeEmbedder.model_id().to_string())
        );
    }

    // ---- M5 slice 1: hash-skip, model-change re-queue, state recovery ----

    fn run(
        conn: &mut Connection,
        root_id: i64,
        root_path: &Path,
        embedder: &dyn TextEmbedder,
        scan_id: i64,
    ) -> IndexSummary {
        index_root(
            conn,
            root_id,
            root_path,
            &default_options(),
            scan_id,
            &IndexContext::new(embedder),
        )
        .unwrap()
    }

    fn scalar(conn: &Connection, sql: &str) -> i64 {
        conn.query_row(sql, [], |r| r.get(0)).unwrap()
    }

    #[test]
    fn an_unchanged_file_is_not_embedded_again() {
        let (_db, root_dir, mut conn, root_id) = open_test_db();
        let root = crate::paths::canonicalize(root_dir.path()).unwrap();
        fs::write(root.join("notes.txt"), "the same words").unwrap();
        let embedder = FakeEmbedder::counting();

        let first = run(&mut conn, root_id, &root, &embedder, 1);
        let embedded = embedder.chunks();
        let second = run(&mut conn, root_id, &root, &embedder, 2);

        assert_eq!((first.indexed, first.unchanged), (1, 0));
        assert_eq!((second.indexed, second.unchanged), (0, 1));
        assert_eq!(embedder.chunks(), embedded, "nothing may be embedded again");
        assert_eq!(scalar(&conn, "SELECT seen_scan_id FROM files"), 2);
    }

    /// SPEC.md §7 M5 item 3.
    #[test]
    fn touching_a_file_without_changing_it_does_not_re_embed() {
        let (_db, root_dir, mut conn, root_id) = open_test_db();
        let root = crate::paths::canonicalize(root_dir.path()).unwrap();
        let file = root.join("notes.txt");
        fs::write(&file, "the same words").unwrap();
        let embedder = FakeEmbedder::counting();
        run(&mut conn, root_id, &root, &embedder, 1);
        let embedded = embedder.chunks();
        let old_mtime = scalar(&conn, "SELECT mtime_ns FROM files");

        // Same bytes, new mtime: what `touch` or a re-save does.
        let later = std::time::SystemTime::now() + std::time::Duration::from_secs(60);
        fs::OpenOptions::new()
            .write(true)
            .open(&file)
            .unwrap()
            .set_modified(later)
            .unwrap();
        let summary = run(&mut conn, root_id, &root, &embedder, 2);

        assert_eq!((summary.indexed, summary.unchanged), (0, 1));
        assert_eq!(embedder.chunks(), embedded);
        assert!(
            scalar(&conn, "SELECT mtime_ns FROM files") > old_mtime,
            "the new mtime must be recorded so the next scan does not hash it again"
        );
    }

    /// SPEC.md §7 M5 item 2: the old chunks, FTS rows and vectors are gone.
    #[test]
    fn a_modified_file_replaces_its_chunks_search_rows_and_vectors() {
        let (_db, root_dir, mut conn, root_id) = open_test_db();
        let root = crate::paths::canonicalize(root_dir.path()).unwrap();
        let file = root.join("notes.txt");
        fs::write(&file, "alpha apple").unwrap();
        let embedder = FakeEmbedder::counting();
        run(&mut conn, root_id, &root, &embedder, 1);
        let embedded = embedder.chunks();

        fs::write(&file, "beta banana and a longer second sentence").unwrap();
        let summary = run(&mut conn, root_id, &root, &embedder, 2);

        assert_eq!(summary.indexed, 1);
        assert!(embedder.chunks() > embedded, "the new content is embedded");
        let fts = |term: &str| {
            crate::search::fts::search_fts(&conn, term, 10)
                .unwrap()
                .len()
        };
        assert_eq!((fts("apple"), fts("banana")), (0, 1));
        assert_eq!(scalar(&conn, "SELECT COUNT(*) FROM files"), 1);
        assert_eq!(
            scalar(&conn, "SELECT COUNT(*) FROM vec_text"),
            scalar(&conn, "SELECT COUNT(*) FROM chunks"),
            "one vector per chunk, none left over from the old content"
        );
    }

    /// SPEC.md §7 M5 item 3b.
    #[test]
    fn a_model_change_re_embeds_unchanged_content_and_the_same_model_does_not() {
        let (_db, root_dir, mut conn, root_id) = open_test_db();
        let root = crate::paths::canonicalize(root_dir.path()).unwrap();
        fs::write(root.join("notes.txt"), "the same words").unwrap();
        let v1 = FakeEmbedder::counting();
        run(&mut conn, root_id, &root, &v1, 1);

        let same = run(&mut conn, root_id, &root, &v1, 2);
        assert_eq!(
            (same.indexed, same.unchanged),
            (0, 1),
            "same id: hash-skip applies"
        );

        let v2 = FakeEmbedder::counting().with_model_id("fake-v2");
        let changed = run(&mut conn, root_id, &root, &v2, 3);
        assert_eq!((changed.indexed, changed.unchanged), (1, 0));
        assert!(
            v2.chunks() > 0,
            "the file must be embedded by the new model"
        );
        assert_eq!(
            db::meta::get(&conn, "text_model_id").unwrap().as_deref(),
            Some("fake-v2")
        );

        let after = run(&mut conn, root_id, &root, &v2, 4);
        assert_eq!(
            (after.indexed, after.unchanged),
            (0, 1),
            "and then it is settled again"
        );
    }

    #[test]
    fn a_stale_pipeline_version_is_re_extracted() {
        let (_db, root_dir, mut conn, root_id) = open_test_db();
        let root = crate::paths::canonicalize(root_dir.path()).unwrap();
        fs::write(root.join("notes.txt"), "the same words").unwrap();
        let embedder = FakeEmbedder::counting();
        run(&mut conn, root_id, &root, &embedder, 1);
        conn.execute("UPDATE files SET pipeline_version = 0", [])
            .unwrap();

        let summary = run(&mut conn, root_id, &root, &embedder, 2);

        assert_eq!((summary.indexed, summary.unchanged), (1, 0));
        assert_eq!(
            scalar(&conn, "SELECT pipeline_version FROM files"),
            PIPELINE_VERSION
        );
    }

    struct NamedOcr(&'static str);
    impl OcrEngine for NamedOcr {
        fn engine_id(&self) -> &str {
            self.0
        }
        fn recognize(&self, _: &image::RgbImage) -> Result<String> {
            Ok(String::new())
        }
    }

    #[test]
    fn an_ocr_engine_change_re_queues_images_and_nothing_else() {
        let (_db, root_dir, mut conn, root_id) = open_test_db();
        let root = crate::paths::canonicalize(root_dir.path()).unwrap();
        fs::write(root.join("notes.txt"), "the same words").unwrap();
        let qr = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/corpus/qr/qr_url.png");
        fs::copy(qr, root.join("code.png")).unwrap();
        let with = |ocr: &'static str| IndexContext {
            ocr: Arc::new(NamedOcr(ocr)),
            ..IndexContext::new(&FakeEmbedder)
        };
        let go = |conn: &mut Connection, ctx: &IndexContext, scan| {
            index_root(conn, root_id, &root, &default_options(), scan, ctx).unwrap()
        };

        go(&mut conn, &with("ocr-a"), 1);
        let same = go(&mut conn, &with("ocr-a"), 2);
        assert_eq!((same.indexed, same.unchanged), (0, 2));

        let changed = go(&mut conn, &with("ocr-b"), 3);
        assert_eq!(
            (changed.indexed, changed.unchanged),
            (1, 1),
            "only the image is read again"
        );
        assert_eq!(
            db::meta::get(&conn, "ocr_engine_id").unwrap().as_deref(),
            Some("ocr-b")
        );
    }

    /// SPEC.md §7 M5 item 14, the part that does not need file permissions: a
    /// file that cannot be read becomes `error` and the others carry on.
    #[test]
    fn a_corrupt_file_is_an_error_and_the_run_continues() {
        let (_db, root_dir, mut conn, root_id) = open_test_db();
        let root = crate::paths::canonicalize(root_dir.path()).unwrap();
        let truncated = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/corpus/edge/truncated.pdf");
        fs::copy(truncated, root.join("a_broken.pdf")).unwrap();
        fs::write(root.join("z_fine.txt"), "still gets indexed").unwrap();

        let summary = run(&mut conn, root_id, &root, &FakeEmbedder, 1);

        assert_eq!((summary.indexed, summary.errored), (1, 1));
        let broken = db::files::get_by_path(&conn, &root.join("a_broken.pdf"))
            .unwrap()
            .unwrap();
        assert_eq!(broken.state, "error");
        assert_eq!(
            crate::search::fts::search_fts(&conn, "indexed", 10)
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn a_pending_file_with_the_same_content_is_confirmed_not_re_extracted() {
        let (_db, root_dir, mut conn, root_id) = open_test_db();
        let root = crate::paths::canonicalize(root_dir.path()).unwrap();
        fs::write(root.join("notes.txt"), "the same words").unwrap();
        let embedder = FakeEmbedder::counting();
        run(&mut conn, root_id, &root, &embedder, 1);
        let embedded = embedder.chunks();
        let id = scalar(&conn, "SELECT id FROM files");
        // What a reconciliation scan does when it sees a different mtime.
        db::files::mark_pending(&conn, id).unwrap();
        conn.execute("UPDATE files SET mtime_ns = 1", []).unwrap();

        let summary = run(&mut conn, root_id, &root, &embedder, 2);

        assert_eq!((summary.indexed, summary.unchanged), (0, 1));
        assert_eq!(embedder.chunks(), embedded);
        let state: String = conn
            .query_row("SELECT state FROM files", [], |r| r.get(0))
            .unwrap();
        assert_eq!(state, "indexed");
    }

    /// The pipeline half of SPEC.md §7 M5 item 11: rows a crash left
    /// `indexing` go back in the queue and are finished.
    #[test]
    fn rows_left_indexing_by_a_crash_are_finished_on_the_next_run() {
        let (_db, root_dir, mut conn, root_id) = open_test_db();
        let root = crate::paths::canonicalize(root_dir.path()).unwrap();
        fs::write(root.join("notes.txt"), "the same words").unwrap();
        run(&mut conn, root_id, &root, &FakeEmbedder, 1);
        conn.execute("UPDATE files SET state = 'indexing'", [])
            .unwrap();

        run(&mut conn, root_id, &root, &FakeEmbedder, 2);

        let state: String = conn
            .query_row("SELECT state FROM files", [], |r| r.get(0))
            .unwrap();
        assert_eq!(state, "indexed");
    }
}
