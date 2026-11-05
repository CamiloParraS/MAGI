//! What reconciliation scans write: new rows, seen marks, moves and deletions.

use std::path::{Path, PathBuf};

use rusqlite::{Connection, params};

use crate::db::roots::indexable_sql;
use crate::error::Result;

use super::FileState;

/// A file the walk found that the index has never seen: a `pending` row with
/// no content hash yet, indexed by whatever the queue does next. `kind` comes
/// from the extension alone; extraction settles it.
pub fn insert_pending(
    conn: &Connection,
    root_id: i64,
    path: &Path,
    rel_path: &Path,
    size: u64,
    mtime_ns: i64,
    scan_id: i64,
) -> Result<()> {
    conn.execute(
        "INSERT INTO files (root_id, path, rel_path, file_name, ext, kind, size, mtime_ns,
                            state, pipeline_version, seen_scan_id)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'pending', 0, ?9)",
        params![
            root_id,
            path.to_string_lossy(),
            rel_path.to_string_lossy(),
            path.file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default(),
            path.extension().and_then(|e| e.to_str()),
            crate::discovery::classify(path, &[]).as_str(),
            size as i64,
            mtime_ns,
            scan_id,
        ],
    )?;
    Ok(())
}

/// A reconciliation scan saw the file and its indexed content is still right.
pub fn mark_seen(conn: &Connection, file_id: i64, scan_id: i64) -> Result<()> {
    conn.execute(
        "UPDATE files SET seen_scan_id = ?2 WHERE id = ?1",
        params![file_id, scan_id],
    )?;
    Ok(())
}

/// A reconciliation scan saw the file differ from its row (or the row is stale):
/// queue it again with its current size and mtime, keeping its chunks
/// searchable until the new ones replace them.
pub fn mark_changed(
    conn: &Connection,
    file_id: i64,
    root_id: i64,
    size: u64,
    mtime_ns: i64,
    scan_id: i64,
) -> Result<()> {
    conn.execute(
        "UPDATE files SET state = 'pending', root_id = ?2, size = ?3, mtime_ns = ?4,
                seen_scan_id = ?5, attempts = 0, next_attempt_at = NULL
         WHERE id = ?1",
        params![file_id, root_id, size as i64, mtime_ns, scan_id],
    )?;
    Ok(())
}

/// Rows of `root_id` that scan `scan_id` did not see.
pub fn unseen(conn: &Connection, root_id: i64, scan_id: i64) -> Result<Vec<i64>> {
    let mut stmt =
        conn.prepare_cached("SELECT id FROM files WHERE root_id = ?1 AND seen_scan_id < ?2")?;
    let rows = stmt
        .query_map(params![root_id, scan_id], |row| row.get(0))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// Rows at `prefix` or below it that scan `scan_id` did not see: what a
/// removed file or folder left behind, or what is no longer wanted.
pub fn unseen_under(conn: &Connection, prefix: &Path, scan_id: i64) -> Result<Vec<i64>> {
    let mut stmt = conn.prepare_cached(
        "SELECT id FROM files
         WHERE seen_scan_id < ?2
           AND (path = ?1 OR substr(path, 1, length(?1) + 1) = ?1 || ?3)",
    )?;
    let rows = stmt
        .query_map(
            params![
                prefix.to_string_lossy(),
                scan_id,
                std::path::MAIN_SEPARATOR.to_string()
            ],
            |row| row.get(0),
        )?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// Whether the row still exists and no scan at or after `scan_id` has seen it.
pub fn is_unseen_since(conn: &Connection, file_id: i64, scan_id: i64) -> Result<bool> {
    Ok(conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM files WHERE id = ?1 AND seen_scan_id < ?2)",
        params![file_id, scan_id],
        |row| row.get(0),
    )?)
}

/// The row's current state, if it still exists.
pub fn state_of(conn: &Connection, file_id: i64) -> Result<Option<FileState>> {
    use rusqlite::OptionalExtension;
    Ok(conn
        .query_row(
            "SELECT state FROM files WHERE id = ?1",
            params![file_id],
            |row| row.get(0),
        )
        .optional()?)
}

pub(super) const HASHED_OF_SIZE_SQL: &str = concat!(
    "SELECT f.id, f.path, f.content_hash FROM files f
     JOIN roots r ON r.id = f.root_id
     WHERE f.size = ?1 AND f.kind = ?2 AND f.content_hash IS NOT NULL AND ",
    indexable_sql!()
);

/// Hashed rows of exactly `size` bytes and `kind` in a usable root, with their
/// path and hash: what a newly found file may have been moved from.
/// Served by the partial `idx_files_size` (migration 0002).
pub fn hashed_of_size(
    conn: &Connection,
    size: u64,
    kind: &str,
) -> Result<Vec<(i64, PathBuf, Vec<u8>)>> {
    let mut stmt = conn.prepare_cached(HASHED_OF_SIZE_SQL)?;
    let rows = stmt
        .query_map(params![size as i64, kind], |row| {
            Ok((
                row.get(0)?,
                PathBuf::from(row.get::<_, String>(1)?),
                row.get(2)?,
            ))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// Where a move lands, for [`rename_file_to`].
pub struct MoveTarget<'a> {
    pub path: &'a Path,
    pub rel_path: &'a Path,
    pub size: u64,
    pub mtime_ns: i64,
}

/// A move or rename: `keep_id` (already indexed) takes over the new location.
/// Chunks and vectors are kept, not recomputed (SPEC.md §5.4 step 4). The
/// filename chunk's text follows the new name so it stays findable by it; its
/// vector still describes the old name. A row in the pipeline goes back to
/// `pending`: its result was read at the old path, so the writer drops it and
/// the file is looked at again where it now is. Runs inside the caller's
/// transaction.
pub fn rename_file_to(
    conn: &Connection,
    keep_id: i64,
    root_id: i64,
    to: MoveTarget<'_>,
    scan_id: i64,
) -> Result<()> {
    conn.execute(
        "UPDATE files SET root_id = ?2, path = ?3, rel_path = ?4, file_name = ?5, ext = ?6,
                size = ?7, mtime_ns = ?8, seen_scan_id = ?9,
                state = CASE state WHEN 'indexing' THEN 'pending' ELSE state END
         WHERE id = ?1",
        params![
            keep_id,
            root_id,
            to.path.to_string_lossy(),
            to.rel_path.to_string_lossy(),
            to.path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default(),
            to.path.extension().and_then(|e| e.to_str()),
            to.size as i64,
            to.mtime_ns,
            scan_id,
        ],
    )?;
    conn.execute(
        "UPDATE chunks SET text = ?2 WHERE file_id = ?1 AND source = 'filename'",
        params![
            keep_id,
            crate::extract::filename::filename_chunk(to.rel_path).text
        ],
    )?;
    Ok(())
}

/// Removes files and everything derived from them: `vec_text` rows (before the
/// chunks their subquery reads), chunks (their `chunks_fts` rows go with them
/// through the trigger), the `vec_image` row, and the file row. Virtual tables
/// are not covered by `FOREIGN KEY` cascades, so each is deleted explicitly
/// (SPEC.md §5.5). **Call inside a transaction.** Returns the thumbnail keys the
/// files had, for [`remove_unreferenced_thumbnails`] after the commit.
pub fn delete_files(conn: &Connection, ids: &[i64]) -> Result<Vec<String>> {
    let mut keys = Vec::new();
    let mut key_of = conn.prepare_cached("SELECT thumb_key FROM files WHERE id = ?1")?;
    let mut vec_text = conn.prepare_cached(
        "DELETE FROM vec_text WHERE chunk_id IN (SELECT id FROM chunks WHERE file_id = ?1)",
    )?;
    let mut vec_image = conn.prepare_cached("DELETE FROM vec_image WHERE file_id = ?1")?;
    let mut chunks = conn.prepare_cached("DELETE FROM chunks WHERE file_id = ?1")?;
    let mut file = conn.prepare_cached("DELETE FROM files WHERE id = ?1")?;
    for &id in ids {
        if let Some(key) = key_of
            .query_row(params![id], |row| row.get::<_, Option<String>>(0))
            .ok()
            .flatten()
        {
            keys.push(key);
        }
        vec_text.execute(params![id])?;
        chunks.execute(params![id])?;
        vec_image.execute(params![id])?;
        file.execute(params![id])?;
    }
    Ok(keys)
}

/// Deletes one file in its own transaction. A file already gone is a no-op.
pub fn delete_file(conn: &mut Connection, file_id: i64) -> Result<()> {
    let tx = conn.transaction()?;
    let keys = delete_files(&tx, &[file_id])?;
    tx.commit()?;
    remove_unreferenced_thumbnails(conn, &keys);
    Ok(())
}

/// Deletes every file under `root_id`. **Call inside a transaction.** Returns
/// the thumbnail keys, as [`delete_files`] does.
pub fn purge_root(conn: &Connection, root_id: i64) -> Result<Vec<String>> {
    let ids: Vec<i64> = conn
        .prepare_cached("SELECT id FROM files WHERE root_id = ?1")?
        .query_map(params![root_id], |row| row.get(0))?
        .collect::<std::result::Result<_, _>>()?;
    delete_files(conn, &ids)
}

/// Thumbnails are keyed by content, so duplicates share one; the file goes only
/// when no remaining row points at it. Best effort: a leftover thumbnail is
/// garbage, not corruption.
pub fn remove_unreferenced_thumbnails(conn: &Connection, keys: &[String]) {
    for key in keys {
        let still_used = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM files WHERE thumb_key = ?1)",
                params![key],
                |row| row.get::<_, bool>(0),
            )
            .unwrap_or(true);
        if !still_used {
            let _ = std::fs::remove_file(crate::thumbs::thumb_path(key));
        }
    }
}
