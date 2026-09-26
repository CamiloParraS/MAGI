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
use crate::features::Feature;

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

fn backfill_key(feature: Feature) -> String {
    format!("backfill_total_{}", feature.as_str())
}

/// Files carrying `feature`'s bit that are still waiting to be processed.
fn backfill_left(conn: &Connection, feature: Feature) -> Result<i64> {
    Ok(conn.query_row(
        "SELECT COUNT(*) FROM files
         WHERE features_missing & ?1 != 0 AND state IN ('pending', 'indexing')",
        [feature.bit()],
        |r| r.get(0),
    )?)
}

/// Re-queues the indexed files that were indexed without a feature that is
/// now running (ADR-0010 backfill) and records the backfill's size for
/// progress. Re-queued files go through the normal pipeline, so their chunks
/// are rebuilt at the real tokenizer's boundaries too. A restart mid-backfill
/// keeps the original total. Returns how many rows were re-queued.
pub fn requeue_missing(conn: &mut Connection, running: &[Feature]) -> Result<usize> {
    let tx = conn.transaction()?;
    let mut marked = 0;
    for &feature in running {
        let key = backfill_key(feature);
        let left = backfill_left(&tx, feature)?;
        let fresh = tx.execute(
            "UPDATE files SET state = 'pending', pipeline_version = 0, attempts = 0,
                              next_attempt_at = NULL
             WHERE state = 'indexed' AND features_missing & ?1 != 0",
            [feature.bit()],
        )? as i64;
        marked += fresh as usize;
        let stored = meta::get(&tx, &key)?.and_then(|v| v.parse::<i64>().ok());
        let total = match stored {
            Some(total) if left > 0 => total + fresh,
            _ => left + fresh,
        };
        if total > 0 {
            meta::set(&tx, &key, &total.to_string())?;
        } else {
            tx.execute("DELETE FROM meta WHERE key = ?1", [&key])?;
        }
    }
    tx.commit()?;
    Ok(marked)
}

/// `(done, total)` while a backfill for `feature` has files left. A file
/// that errors counts as done: its retry is the error list's job.
pub fn backfill_progress(conn: &Connection, feature: Feature) -> Result<Option<(u64, u64)>> {
    let Some(total) = meta::get(conn, &backfill_key(feature))?.and_then(|v| v.parse::<u64>().ok())
    else {
        return Ok(None);
    };
    let left = backfill_left(conn, feature)? as u64;
    Ok((left > 0).then(|| (total.saturating_sub(left), total)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::Feature;

    /// Three indexed files missing meaning, in one root.
    fn db_with_three_missing_meaning() -> (tempfile::TempDir, Connection) {
        let dir = tempfile::tempdir().unwrap();
        let conn = crate::db::open(&dir.path().join("magi.db")).unwrap();
        conn.execute(
            "INSERT INTO roots (id, path, added_at) VALUES (1, '/r', 0)",
            [],
        )
        .unwrap();
        for i in 0..3 {
            conn.execute(
                "INSERT INTO files (root_id, path, rel_path, file_name, kind, size, mtime_ns,
                                    state, seen_scan_id, pipeline_version, features_missing)
                 VALUES (1, ?1, ?1, ?1, 'text', 1, 0, 'indexed', 0, ?2, 1)",
                rusqlite::params![format!("/r/{i}.txt"), PIPELINE_VERSION],
            )
            .unwrap();
        }
        (dir, conn)
    }

    fn set(conn: &Connection, path: &str, state: &str, missing: i64) {
        conn.execute(
            "UPDATE files SET state = ?2, features_missing = ?3 WHERE path = ?1",
            rusqlite::params![path, state, missing],
        )
        .unwrap();
    }

    #[test]
    fn backfill_progress_counts_down_and_ignores_failed_files() {
        let (_dir, mut conn) = db_with_three_missing_meaning();
        assert_eq!(backfill_progress(&conn, Feature::Meaning).unwrap(), None);

        assert_eq!(requeue_missing(&mut conn, &[Feature::Meaning]).unwrap(), 3);
        assert_eq!(
            backfill_progress(&conn, Feature::Meaning).unwrap(),
            Some((0, 3))
        );
        assert_eq!(backfill_progress(&conn, Feature::ImageText).unwrap(), None);

        set(&conn, "/r/0.txt", "indexed", 0);
        set(&conn, "/r/1.txt", "error", 1);
        assert_eq!(
            backfill_progress(&conn, Feature::Meaning).unwrap(),
            Some((2, 3)),
            "an error is done, not stuck"
        );

        // Restarted mid-backfill: the total is kept, nothing new is queued.
        assert_eq!(requeue_missing(&mut conn, &[Feature::Meaning]).unwrap(), 0);
        assert_eq!(
            backfill_progress(&conn, Feature::Meaning).unwrap(),
            Some((2, 3))
        );

        set(&conn, "/r/2.txt", "indexed", 0);
        assert_eq!(
            backfill_progress(&conn, Feature::Meaning).unwrap(),
            None,
            "finished"
        );

        // A later backfill starts from its own total.
        set(&conn, "/r/0.txt", "indexed", 1);
        requeue_missing(&mut conn, &[Feature::Meaning]).unwrap();
        assert_eq!(
            backfill_progress(&conn, Feature::Meaning).unwrap(),
            Some((0, 1))
        );
    }

    #[test]
    fn a_feature_that_is_not_running_queues_nothing() {
        let (_dir, mut conn) = db_with_three_missing_meaning();
        assert_eq!(
            requeue_missing(&mut conn, &[Feature::ImageText]).unwrap(),
            0
        );
        assert_eq!(requeue_missing(&mut conn, &[]).unwrap(), 0);
    }
}
