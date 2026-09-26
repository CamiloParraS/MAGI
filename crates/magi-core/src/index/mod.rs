//! Indexing: scheduler, pipeline, and DB writer. Implemented starting M2
//! (see SPEC.md §5.3 threads, §7).

pub(crate) mod change;
pub mod gate;
mod isolate;
pub(crate) mod lifecycle;
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
/// before M5 carry `0` and are re-queued once. 2: merged code symbols, data
/// files capped to their first 64 KB.
pub const PIPELINE_VERSION: i64 = 2;

/// The identifiers of the components that produced a file's index rows. A
/// stored id that differs from the running one means those rows are stale.
/// Each is `None` when that component is not running: its rows are left as
/// they are rather than judged against a model that is not running.
pub struct ModelIds<'a> {
    pub text: Option<&'a str>,
    pub image: Option<&'a str>,
    pub ocr: Option<&'a str>,
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

    if let Some(text) = ids.text
        && differs("text_model_id", text)?
    {
        tracing::info!(
            current = text,
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
    if let Some(ocr) = ids.ocr
        && differs("ocr_engine_id", ocr)?
    {
        tracing::info!(current = ocr, "OCR engine changed; re-reading images");
        marked += files::invalidate(&tx, Some("image"))?;
    }

    for (key, id) in [
        ("text_model_id", ids.text),
        ("image_model_id", ids.image),
        ("ocr_engine_id", ids.ocr),
    ] {
        if let Some(id) = id {
            meta::set(&tx, key, id)?;
        }
    }
    tx.commit()?;
    Ok(marked)
}
