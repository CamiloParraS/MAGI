//! Indexing: scheduler, pipeline, and DB writer. Implemented starting M2
//! (see SPEC.md §5.3 threads, §7).

pub mod gate;
mod isolate;
pub mod pipeline;
pub mod resources;
pub mod scheduler;
pub mod writer;

use rusqlite::Connection;

use crate::db::{files, meta};
use crate::error::Result;

/// Bumped whenever extraction or chunking changes what a file's index rows
/// would contain, so files indexed by older code are re-extracted instead of
/// being kept by the hash-skip (SPEC.md §5.4, "Versioning"). Rows written
/// before M5 carry `0` and are re-queued once.
pub const PIPELINE_VERSION: i64 = 1;

/// The identifiers of the components that produced a file's index rows. A
/// stored id that differs from the running one means those rows are stale.
pub struct ModelIds<'a> {
    pub text: &'a str,
    /// `None` when no image model is configured: image vectors are left as
    /// they are rather than judged against a model that is not running.
    pub image: Option<&'a str>,
    pub ocr: &'a str,
}

/// Re-queues files whose text vectors, image vectors or OCR text came from a
/// different model or engine than the running one (SPEC.md §7 M5). Content
/// hashes are untouched, so the hash-skip alone would keep the stale rows;
/// [`files::invalidate`] sets `pipeline_version = 0` to defeat it. Old rows
/// stay searchable until the new ones replace them. The stored ids are updated
/// in the same transaction, so a crash cannot lose the fact that files were
/// re-queued: the `pending` rows themselves are the record.
///
/// A missing stored id (first run, or a database from before M5) is not a
/// mismatch: there is nothing recorded to disagree with.
///
/// Returns how many file rows were marked.
pub fn requeue_on_model_change(conn: &mut Connection, ids: &ModelIds) -> Result<usize> {
    let tx = conn.transaction()?;
    let mut marked = 0;
    let differs = |key: &str, current: &str| -> Result<bool> {
        Ok(meta::get(&tx, key)?.is_some_and(|stored| stored != current))
    };

    if differs("text_model_id", ids.text)? {
        tracing::info!(
            current = ids.text,
            "text model changed; re-embedding every file"
        );
        marked += files::invalidate(&tx, None)?;
    }
    if let Some(image) = ids.image
        && differs("image_model_id", image)?
    {
        tracing::info!(current = image, "image model changed; re-embedding images");
        marked += files::invalidate(&tx, Some("image"))?;
    }
    if differs("ocr_engine_id", ids.ocr)? {
        tracing::info!(current = ids.ocr, "OCR engine changed; re-reading images");
        marked += files::invalidate(&tx, Some("image"))?;
    }

    meta::set(&tx, "text_model_id", ids.text)?;
    if let Some(image) = ids.image {
        meta::set(&tx, "image_model_id", image)?;
    }
    meta::set(&tx, "ocr_engine_id", ids.ocr)?;
    tx.commit()?;
    Ok(marked)
}
