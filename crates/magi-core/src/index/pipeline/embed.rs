//! Stages 2 and 3: embed a file's chunks, then store it in one transaction.

use std::path::Path;

use rusqlite::Connection;

use crate::db::files::{FileRecord, FileState, upsert_file};
use crate::dto::FileErrorCode;
use crate::embed::TextEmbedder;
use crate::error::Result;
use crate::extract::RawChunk;

use super::extract::FileOutcome;
use super::{Embedded, Fresh, IndexContext, Job, Status};

/// Stage 2, embed. `before_batch` runs ahead of every batch: the runtime uses it
/// to yield to a search that is waiting for the model.
pub(crate) fn embed(
    ctx: &IndexContext,
    job: &Job,
    mut fresh: Fresh,
    before_batch: &dyn Fn(),
) -> Result<Embedded> {
    let embeddings = match &ctx.embedder {
        Some(embedder) => Some(embed_chunks(
            &**embedder,
            &job.entry.path,
            &mut fresh.chunks,
            &mut fresh.outcome,
            before_batch,
        )?),
        None => None,
    };
    Ok(Embedded { fresh, embeddings })
}

/// Stage 2 for several files at once: small files fill a model call together
/// instead of each paying for a nearly empty one. Chunks go shortest first and
/// a batch ends where lengths jump ([`batches_by_length`]): the model pads a
/// batch to its longest input, so a filename chunk batched with a full body
/// chunk would cost as much as one. If a batch fails, every file is embedded
/// on its own instead ([`embed`]), so a bad file costs only itself.
pub(crate) fn embed_group(
    ctx: &IndexContext,
    group: Vec<(Job, Fresh)>,
    before_batch: &dyn Fn(),
) -> Vec<(Job, Result<Embedded>)> {
    let Some(embedder) = ctx.embedder.as_deref() else {
        return group
            .into_iter()
            .map(|(job, fresh)| {
                let embedded = Embedded {
                    fresh,
                    embeddings: None,
                };
                (job, Ok(embedded))
            })
            .collect();
    };
    let chunk = |(f, c): (usize, usize)| group[f].1.chunks[c].text.as_str();
    let mut order: Vec<(usize, usize)> = group
        .iter()
        .enumerate()
        .flat_map(|(f, (_, fresh))| (0..fresh.chunks.len()).map(move |c| (f, c)))
        .collect();
    order.sort_by_key(|&at| chunk(at).len());

    let mut vectors: Vec<Vec<Vec<f32>>> = group
        .iter()
        .map(|(_, fresh)| vec![Vec::new(); fresh.chunks.len()])
        .collect();
    for batch in batches_by_length(&order, |at| chunk(at).len()) {
        before_batch();
        let texts: Vec<&str> = batch.iter().map(|&at| chunk(at)).collect();
        match embedder.embed_passages(&texts) {
            Ok(out) if out.len() == texts.len() => {
                for (&(f, c), v) in batch.iter().zip(out) {
                    vectors[f][c] = v;
                }
            }
            _ => {
                return group
                    .into_iter()
                    .map(|(job, fresh)| {
                        let embedded = embed(ctx, &job, fresh, before_batch);
                        (job, embedded)
                    })
                    .collect();
            }
        }
    }
    group
        .into_iter()
        .zip(vectors)
        .map(|((job, fresh), embeddings)| {
            let embedded = Embedded {
                fresh,
                embeddings: Some(embeddings),
            };
            (job, Ok(embedded))
        })
        .collect()
}

/// Splits `sorted` (shortest first) into batches of at most [`EMBED_BATCH`],
/// starting a new one when an item is over twice as long as the batch's first.
/// Lengths are bytes, a stand-in for tokens; under [`SHORT_BYTES`] counts as
/// short, so names and one-liners share a batch.
pub(super) fn batches_by_length<T: Copy>(sorted: &[T], len: impl Fn(T) -> usize) -> Vec<&[T]> {
    const SHORT_BYTES: usize = 64;
    let mut batches = Vec::new();
    let mut start = 0;
    for i in 1..=sorted.len() {
        let ends = i == sorted.len()
            || i - start == EMBED_BATCH
            || len(sorted[i]) > 2 * len(sorted[start]).max(SHORT_BYTES);
        if ends {
            batches.push(&sorted[start..i]);
            start = i;
        }
    }
    batches
}

/// Stage 3, store: one transaction per file.
pub(crate) fn store_embedded(
    conn: &mut Connection,
    job: &Job,
    embedded: Embedded,
) -> Result<Status> {
    let Embedded {
        fresh:
            Fresh {
                outcome,
                chunks,
                thumb_key,
                features_missing,
            },
        embeddings,
    } = embedded;
    let entry = &job.entry;
    let rel_path = entry
        .path
        .strip_prefix(&job.root_path)
        .unwrap_or(&entry.path);
    let file_name = entry
        .path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default();
    let ext = entry.path.extension().and_then(|e| e.to_str());
    let record = FileRecord {
        root_id: job.root_id,
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
        error: outcome
            .error
            .as_ref()
            .map(|(code, text)| (*code, text.as_str())),
        // Only inserts use it, and the row exists (a result for a vanished
        // row is dropped): scans own `seen_scan_id`.
        seen_scan_id: 0,
        content_hash: outcome.content_hash.as_ref().map(|h| h.as_slice()),
        thumb_key: thumb_key.as_deref(),
        features_missing,
    };
    upsert_file(
        conn,
        &record,
        &chunks,
        embeddings.as_deref(),
        outcome.doc.image_embedding.as_deref(),
    )?;
    Ok(match outcome.state {
        FileState::Indexed => Status::Indexed,
        FileState::Skipped => Status::Skipped,
        FileState::Error | FileState::Pending | FileState::Indexing => Status::Errored,
    })
}

/// Embeds `chunks` (content chunks, filename chunk last) in batches. One bad
/// file must not abort the run: if embedding fails, the file keeps only its
/// filename chunk (still searchable by name) and is marked `error`. If even
/// that fails, the embedder itself is broken and the error propagates rather
/// than erroring every remaining file.
fn embed_chunks(
    embedder: &dyn TextEmbedder,
    path: &Path,
    chunks: &mut Vec<RawChunk>,
    outcome: &mut FileOutcome,
    before_batch: &dyn Fn(),
) -> Result<Vec<Vec<f32>>> {
    let texts: Vec<&str> = chunks.iter().map(|c| c.text.as_str()).collect();
    let mut embeddings = Vec::with_capacity(texts.len());
    for batch in texts.chunks(EMBED_BATCH) {
        before_batch();
        match embedder.embed_passages(batch) {
            Ok(mut vectors) => embeddings.append(&mut vectors),
            Err(e) => {
                tracing::warn!(path = %path.display(), error = %e, "embedding failed");
                outcome.state = FileState::Error;
                outcome.error = Some((FileErrorCode::EmbedFailed, e.to_string()));
                chunks.drain(..chunks.len() - 1);
                return embedder.embed_passages(&[chunks[0].text.as_str()]);
            }
        }
    }
    Ok(embeddings)
}

/// Chunks per model call: small enough that a waiting search is served
/// between batches (SPEC.md §5.3 priority lock).
pub(crate) const EMBED_BATCH: usize = 16;
