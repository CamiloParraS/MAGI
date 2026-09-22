//! File/chunk repositories. Implemented in M1+ (see SPEC.md §5.5).

use std::path::{Path, PathBuf};

use rusqlite::{Connection, params};

use crate::embed::embedding_to_json;
use crate::error::Result;
use crate::extract::RawChunk;

/// Everything needed to upsert one file's row.
pub struct FileRecord<'a> {
    pub root_id: i64,
    pub path: &'a Path,
    pub rel_path: &'a Path,
    pub file_name: &'a str,
    pub ext: Option<&'a str>,
    pub kind: &'a str,
    pub size: u64,
    pub mtime_ns: i64,
    pub lang: Option<&'a str>,
    pub state: &'a str,
    pub skip_reason: Option<&'a str>,
    pub error: Option<&'a str>,
    pub seen_scan_id: i64,
    /// blake3 of the file's bytes; `None` when the content was never read.
    pub content_hash: Option<&'a [u8]>,
    /// Thumbnail cache key (see `crate::thumbs`).
    pub thumb_key: Option<&'a str>,
}

/// Inserts or replaces `record`, its `chunks`, and their `vec_text`
/// embeddings in one transaction: the file row is upserted keyed on its
/// unique `path`, and any previous chunks/vectors for that file are
/// deleted before the new ones are inserted (idempotent re-indexing).
/// `embeddings[i]` is the vector for `chunks[i]` — same length, same order.
/// `image_embedding` replaces the file's `vec_image` row (or removes it
/// when `None`) in the same transaction.
///
/// Deletes `vec_text` rows before `chunks` (not after): the delete uses a
/// subquery over `chunks` to find which `vec_text` rows belong to this
/// file, so it must run while those `chunks` rows still exist (SPEC.md
/// §5.5: "Deletions MUST explicitly delete `vec_*` rows ... in the same
/// transaction" — virtual tables aren't covered by `FOREIGN KEY` cascades).
pub fn upsert_file(
    conn: &mut Connection,
    record: &FileRecord,
    chunks: &[RawChunk],
    embeddings: &[Vec<f32>],
    image_embedding: Option<&[f32]>,
) -> Result<i64> {
    if chunks.len() != embeddings.len() {
        // `zip` below would silently drop the excess, i.e. lose chunks
        // from the index with no error anywhere.
        return Err(crate::error::Error::Model(format!(
            "embedder returned {} embeddings for {}'s {} chunks",
            embeddings.len(),
            record.path.display(),
            chunks.len(),
        )));
    }
    let tx = conn.transaction()?;
    let path_str = record.path.to_string_lossy();
    let rel_path_str = record.rel_path.to_string_lossy();

    let file_id: i64 = tx.query_row(
        "INSERT INTO files (
            root_id, path, rel_path, file_name, ext, kind, size, mtime_ns,
            lang, state, skip_reason, error, pipeline_version, seen_scan_id, indexed_at,
            content_hash, thumb_key
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?16, ?13, unixepoch(), ?14, ?15)
         ON CONFLICT(path) DO UPDATE SET
            root_id = excluded.root_id,
            rel_path = excluded.rel_path,
            file_name = excluded.file_name,
            ext = excluded.ext,
            kind = excluded.kind,
            size = excluded.size,
            mtime_ns = excluded.mtime_ns,
            lang = excluded.lang,
            state = excluded.state,
            skip_reason = excluded.skip_reason,
            error = excluded.error,
            seen_scan_id = excluded.seen_scan_id,
            content_hash = excluded.content_hash,
            thumb_key = excluded.thumb_key,
            pipeline_version = excluded.pipeline_version,
            attempts = 0,
            next_attempt_at = NULL,
            indexed_at = unixepoch()
         RETURNING id",
        params![
            record.root_id,
            path_str,
            rel_path_str,
            record.file_name,
            record.ext,
            record.kind,
            record.size as i64,
            record.mtime_ns,
            record.lang,
            record.state,
            record.skip_reason,
            record.error,
            record.seen_scan_id,
            record.content_hash,
            record.thumb_key,
            crate::index::PIPELINE_VERSION,
        ],
        |row| row.get(0),
    )?;

    tx.execute(
        "DELETE FROM vec_text WHERE chunk_id IN (SELECT id FROM chunks WHERE file_id = ?1)",
        params![file_id],
    )?;
    tx.execute("DELETE FROM chunks WHERE file_id = ?1", params![file_id])?;
    tx.execute("DELETE FROM vec_image WHERE file_id = ?1", params![file_id])?;
    if let Some(embedding) = image_embedding {
        tx.execute(
            "INSERT INTO vec_image (file_id, embedding) VALUES (?1, vec_f32(?2))",
            params![file_id, embedding_to_json(embedding)],
        )?;
    }
    // Prepared once, not per chunk: a single large file can carry
    // thousands. `RETURNING id` avoids a follow-up SELECT — and avoids
    // `tx.last_insert_rowid()`, which the `chunks_ai` trigger's own INSERT
    // into `chunks_fts` would shadow.
    {
        let mut insert_chunk = tx.prepare(
            "INSERT INTO chunks (file_id, ordinal, source, text, page, line_start, line_end)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             RETURNING id",
        )?;
        let mut insert_vector =
            tx.prepare("INSERT INTO vec_text (chunk_id, embedding) VALUES (?1, vec_f32(?2))")?;
        for (ordinal, (chunk, embedding)) in chunks.iter().zip(embeddings).enumerate() {
            let chunk_id: i64 = insert_chunk.query_row(
                params![
                    file_id,
                    ordinal as i64,
                    chunk.source.as_str(),
                    chunk.text,
                    chunk.page,
                    chunk.line_start,
                    chunk.line_end,
                ],
                |row| row.get(0),
            )?;
            insert_vector.execute(params![chunk_id, embedding_to_json(embedding)])?;
        }
    }

    tx.commit()?;
    Ok(file_id)
}

pub struct FileRow {
    pub id: i64,
    pub path: PathBuf,
    pub kind: String,
    pub state: String,
    pub skip_reason: Option<String>,
    pub lang: Option<String>,
}

pub fn get_by_path(conn: &Connection, path: &Path) -> Result<Option<FileRow>> {
    use rusqlite::OptionalExtension;
    conn.query_row(
        "SELECT id, path, kind, state, skip_reason, lang FROM files WHERE path = ?1",
        params![path.to_string_lossy()],
        |row| {
            Ok(FileRow {
                id: row.get(0)?,
                path: PathBuf::from(row.get::<_, String>(1)?),
                kind: row.get(2)?,
                state: row.get(3)?,
                skip_reason: row.get(4)?,
                lang: row.get(5)?,
            })
        },
    )
    .optional()
    .map_err(crate::error::Error::Db)
}

/// Attempts before a file goes to `error` (SPEC.md §5.4).
pub const MAX_ATTEMPTS: u32 = 3;

/// Delay before the next try after the `attempt`-th failure: 30 s, 2 min,
/// then 10 min (SPEC.md §5.4 step 6). The last value repeats.
pub fn backoff_secs(attempt: u32) -> i64 {
    const SECS: [i64; 3] = [30, 120, 600];
    SECS[(attempt.saturating_sub(1) as usize).min(SECS.len() - 1)]
}

/// The columns change detection needs about a file already in the index.
pub struct StoredFile {
    pub id: i64,
    pub root_id: i64,
    pub kind: String,
    pub state: String,
    pub skip_reason: Option<String>,
    pub size: u64,
    pub mtime_ns: i64,
    pub content_hash: Option<Vec<u8>>,
    pub pipeline_version: i64,
}

pub fn get_stored(conn: &Connection, path: &Path) -> Result<Option<StoredFile>> {
    use rusqlite::OptionalExtension;
    conn.query_row(
        "SELECT id, root_id, kind, state, skip_reason, size, mtime_ns, content_hash,
                pipeline_version
         FROM files WHERE path = ?1",
        params![path.to_string_lossy()],
        |row| {
            Ok(StoredFile {
                id: row.get(0)?,
                root_id: row.get(1)?,
                kind: row.get(2)?,
                state: row.get(3)?,
                skip_reason: row.get(4)?,
                size: row.get::<_, i64>(5)? as u64,
                mtime_ns: row.get(6)?,
                content_hash: row.get(7)?,
                pipeline_version: row.get(8)?,
            })
        },
    )
    .optional()
    .map_err(crate::error::Error::Db)
}

/// Records that a file was looked at and its indexed content is still right:
/// only `size`, `mtime_ns`, the scan it was seen in and its `state` change, so
/// nothing is re-extracted or re-embedded (SPEC.md §5.4 step 4).
pub fn touch_unchanged(
    conn: &Connection,
    file_id: i64,
    size: u64,
    mtime_ns: i64,
    scan_id: i64,
    state: &str,
) -> Result<()> {
    conn.execute(
        "UPDATE files SET size = ?2, mtime_ns = ?3, seen_scan_id = ?4, state = ?5,
                attempts = 0, next_attempt_at = NULL
         WHERE id = ?1",
        params![file_id, size as i64, mtime_ns, scan_id, state],
    )?;
    Ok(())
}

/// Puts a file back in the queue, keeping its chunks searchable until the new
/// ones replace them.
pub fn mark_pending(conn: &Connection, file_id: i64) -> Result<()> {
    conn.execute(
        "UPDATE files SET state = 'pending', attempts = 0, next_attempt_at = NULL WHERE id = ?1",
        params![file_id],
    )?;
    Ok(())
}

pub fn mark_indexing(conn: &Connection, file_id: i64) -> Result<()> {
    conn.execute(
        "UPDATE files SET state = 'indexing' WHERE id = ?1",
        params![file_id],
    )?;
    Ok(())
}

/// Startup step 1 (SPEC.md §5.4): a crash mid-index leaves rows `indexing`;
/// nothing is working on them now, so they go back in the queue.
pub fn reset_indexing_to_pending(conn: &Connection) -> Result<usize> {
    Ok(conn.execute(
        "UPDATE files SET state = 'pending' WHERE state = 'indexing'",
        [],
    )?)
}

/// Marks files whose vectors or text came from an older model or pipeline as
/// needing a full re-extract: `pipeline_version = 0` is what stops the
/// hash-skip from preserving them. Restricted to one `kind` when given.
pub fn invalidate(conn: &Connection, kind: Option<&str>) -> Result<usize> {
    Ok(conn.execute(
        "UPDATE files SET state = 'pending', pipeline_version = 0, attempts = 0,
                next_attempt_at = NULL
         WHERE ?1 IS NULL OR kind = ?1",
        params![kind],
    )?)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingFile {
    pub id: i64,
    pub root_id: i64,
    pub path: PathBuf,
    pub size: u64,
    pub mtime_ns: i64,
}

/// The next files to work on: `pending`, not waiting out a retry delay, most
/// recently modified first so fresh files become searchable first (SPEC.md
/// §5.4 step 6).
pub fn next_pending(conn: &Connection, now: i64, limit: u32) -> Result<Vec<PendingFile>> {
    let mut stmt = conn.prepare_cached(
        "SELECT id, root_id, path, size, mtime_ns FROM files
         WHERE state = 'pending' AND (next_attempt_at IS NULL OR next_attempt_at <= ?1)
         ORDER BY mtime_ns DESC, id
         LIMIT ?2",
    )?;
    let rows = stmt
        .query_map(params![now, limit], |row| {
            Ok(PendingFile {
                id: row.get(0)?,
                root_id: row.get(1)?,
                path: PathBuf::from(row.get::<_, String>(2)?),
                size: row.get::<_, i64>(3)? as u64,
                mtime_ns: row.get(4)?,
            })
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// What happened to a file after a failed attempt.
#[derive(Debug, PartialEq, Eq)]
pub enum Failure {
    /// Back in the queue, not to be tried before `next_attempt_at` (unix seconds).
    Retry { attempts: u32, next_attempt_at: i64 },
    /// [`MAX_ATTEMPTS`] reached: the file is `error` until retried by hand.
    GaveUp { attempts: u32 },
}

pub fn record_failure(conn: &Connection, file_id: i64, message: &str, now: i64) -> Result<Failure> {
    let attempts: u32 = conn.query_row(
        "SELECT attempts FROM files WHERE id = ?1",
        params![file_id],
        |row| row.get::<_, u32>(0),
    )? + 1;
    if attempts >= MAX_ATTEMPTS {
        conn.execute(
            "UPDATE files SET state = 'error', attempts = ?2, error = ?3, next_attempt_at = NULL
             WHERE id = ?1",
            params![file_id, attempts, message],
        )?;
        return Ok(Failure::GaveUp { attempts });
    }
    let next_attempt_at = now + backoff_secs(attempts);
    conn.execute(
        "UPDATE files SET state = 'pending', attempts = ?2, error = ?3, next_attempt_at = ?4
         WHERE id = ?1",
        params![file_id, attempts, message, next_attempt_at],
    )?;
    Ok(Failure::Retry {
        attempts,
        next_attempt_at,
    })
}

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

/// A file the last scan no longer found, and what move detection needs to know
/// about it.
pub struct Missing {
    pub id: i64,
    pub kind: String,
    pub size: u64,
    pub content_hash: Option<Vec<u8>>,
}

/// Rows of `root_id` that scan `scan_id` did not see.
pub fn unseen(conn: &Connection, root_id: i64, scan_id: i64) -> Result<Vec<Missing>> {
    let mut stmt = conn.prepare_cached(
        "SELECT id, kind, size, content_hash FROM files
         WHERE root_id = ?1 AND seen_scan_id < ?2",
    )?;
    let rows = stmt
        .query_map(params![root_id, scan_id], |row| {
            Ok(Missing {
                id: row.get(0)?,
                kind: row.get(1)?,
                size: row.get::<_, i64>(2)? as u64,
                content_hash: row.get(3)?,
            })
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// Rows at `prefix` or below it that scan `scan_id` did not see: what a
/// removed file or folder left behind, or what is no longer wanted.
pub fn unseen_under(conn: &Connection, prefix: &Path, scan_id: i64) -> Result<Vec<Missing>> {
    let mut stmt = conn.prepare_cached(
        "SELECT id, kind, size, content_hash FROM files
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
            |row| {
                Ok(Missing {
                    id: row.get(0)?,
                    kind: row.get(1)?,
                    size: row.get::<_, i64>(2)? as u64,
                    content_hash: row.get(3)?,
                })
            },
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
pub fn state_of(conn: &Connection, file_id: i64) -> Result<Option<String>> {
    use rusqlite::OptionalExtension;
    Ok(conn
        .query_row(
            "SELECT state FROM files WHERE id = ?1",
            params![file_id],
            |row| row.get(0),
        )
        .optional()?)
}

/// Brand-new `pending` rows (no hash yet) of exactly `size` bytes: the only
/// places a moved file can have turned up.
pub fn new_pending_of_size(conn: &Connection, size: u64) -> Result<Vec<(i64, PathBuf)>> {
    let mut stmt = conn.prepare_cached(
        "SELECT id, path FROM files
         WHERE state = 'pending' AND content_hash IS NULL AND size = ?1",
    )?;
    let rows = stmt
        .query_map(params![size as i64], |row| {
            Ok((row.get(0)?, PathBuf::from(row.get::<_, String>(1)?)))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// A move or rename: `keep_id` (already indexed) takes over the location of
/// `new_id` (the fresh `pending` row for the same bytes), which is dropped.
/// Chunks and vectors are kept, not recomputed (SPEC.md §5.4 step 4). The
/// filename chunk's text follows the new name so it stays findable by it; its
/// vector still describes the old name.
pub fn rename_file(conn: &mut Connection, keep_id: i64, new_id: i64, scan_id: i64) -> Result<()> {
    let tx = conn.transaction()?;
    let (root_id, path, rel_path, file_name, ext, size, mtime_ns): (
        i64,
        String,
        String,
        String,
        Option<String>,
        i64,
        i64,
    ) = tx.query_row(
        "SELECT root_id, path, rel_path, file_name, ext, size, mtime_ns FROM files WHERE id = ?1",
        params![new_id],
        |r| {
            Ok((
                r.get(0)?,
                r.get(1)?,
                r.get(2)?,
                r.get(3)?,
                r.get(4)?,
                r.get(5)?,
                r.get(6)?,
            ))
        },
    )?;
    delete_files(&tx, &[new_id])?;
    tx.execute(
        "UPDATE files SET root_id = ?2, path = ?3, rel_path = ?4, file_name = ?5, ext = ?6,
                size = ?7, mtime_ns = ?8, seen_scan_id = ?9
         WHERE id = ?1",
        params![
            keep_id, root_id, path, rel_path, file_name, ext, size, mtime_ns, scan_id
        ],
    )?;
    tx.execute(
        "UPDATE chunks SET text = ?2 WHERE file_id = ?1 AND source = 'filename'",
        params![
            keep_id,
            crate::extract::filename::filename_chunk(Path::new(&rel_path)).text
        ],
    )?;
    tx.commit()?;
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

/// Deletes one file in its own transaction. `Ok(false)` if it was not there.
pub fn delete_file(conn: &mut Connection, file_id: i64) -> Result<bool> {
    let tx = conn.transaction()?;
    let existed: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM files WHERE id = ?1)",
        params![file_id],
        |row| row.get(0),
    )?;
    let keys = delete_files(&tx, &[file_id])?;
    tx.commit()?;
    remove_unreferenced_thumbnails(conn, &keys);
    Ok(existed)
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

pub fn count_files(conn: &Connection) -> Result<i64> {
    Ok(conn.query_row("SELECT COUNT(*) FROM files", [], |row| row.get(0))?)
}

pub fn count_chunks(conn: &Connection) -> Result<i64> {
    Ok(conn.query_row("SELECT COUNT(*) FROM chunks", [], |row| row.get(0))?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn upsert(
        conn: &mut Connection,
        record: &FileRecord,
        chunks: &[RawChunk],
        embeddings: &[Vec<f32>],
    ) -> Result<i64> {
        upsert_file(conn, record, chunks, embeddings, None)
    }
    use crate::db;
    use crate::embed::{FakeEmbedder, TextEmbedder};
    use crate::extract::ChunkSource;

    fn open_test_db() -> (tempfile::TempDir, Connection) {
        let dir = tempfile::tempdir().unwrap();
        let conn = db::open(&dir.path().join("magi.db")).unwrap();
        let root_dir = tempfile::tempdir().unwrap();
        db::roots::add(&conn, root_dir.path()).unwrap();
        (dir, conn)
    }

    fn fake_embeddings(chunks: &[RawChunk]) -> Vec<Vec<f32>> {
        let texts: Vec<&str> = chunks.iter().map(|c| c.text.as_str()).collect();
        FakeEmbedder.embed_passages(&texts).unwrap()
    }

    fn sample_record<'a>(path: &'a Path, rel_path: &'a Path) -> FileRecord<'a> {
        FileRecord {
            root_id: 1,
            path,
            rel_path,
            file_name: "notes.txt",
            ext: Some("txt"),
            kind: "text",
            size: 11,
            mtime_ns: 123,
            lang: Some("en"),
            state: "indexed",
            skip_reason: None,
            error: None,
            seen_scan_id: 1,
            content_hash: None,
            thumb_key: None,
        }
    }

    #[test]
    fn inserts_file_and_chunks_and_populates_fts() {
        let (_dir, mut conn) = open_test_db();
        let path = PathBuf::from("/roots/a/notes.txt");
        let rel = PathBuf::from("notes.txt");
        let chunks = vec![RawChunk::body("hello world".to_string())];

        let file_id = upsert(
            &mut conn,
            &sample_record(&path, &rel),
            &chunks,
            &fake_embeddings(&chunks),
        )
        .unwrap();

        assert_eq!(count_files(&conn).unwrap(), 1);
        assert_eq!(count_chunks(&conn).unwrap(), 1);

        let matched: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM chunks_fts WHERE chunks_fts MATCH 'hello'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(matched, 1);

        let row = get_by_path(&conn, &path).unwrap().unwrap();
        assert_eq!(row.id, file_id);
        assert_eq!(row.state, "indexed");
    }

    #[test]
    fn reindexing_same_path_replaces_chunks_without_duplicating_rows() {
        let (_dir, mut conn) = open_test_db();
        let path = PathBuf::from("/roots/a/notes.txt");
        let rel = PathBuf::from("notes.txt");

        let first_chunks = vec![RawChunk::body("version one".to_string())];
        let first_id = upsert(
            &mut conn,
            &sample_record(&path, &rel),
            &first_chunks,
            &fake_embeddings(&first_chunks),
        )
        .unwrap();
        let second_chunks = vec![
            RawChunk::body("version two".to_string()),
            RawChunk::body("more text".to_string()),
        ];
        let second_id = upsert(
            &mut conn,
            &sample_record(&path, &rel),
            &second_chunks,
            &fake_embeddings(&second_chunks),
        )
        .unwrap();

        assert_eq!(first_id, second_id);
        assert_eq!(count_files(&conn).unwrap(), 1);
        assert_eq!(count_chunks(&conn).unwrap(), 2);

        let stale: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM chunks_fts WHERE chunks_fts MATCH 'one'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(stale, 0);
    }

    #[test]
    fn skipped_file_records_reason() {
        let (_dir, mut conn) = open_test_db();
        let path = PathBuf::from("/roots/a/huge.bin");
        let rel = PathBuf::from("huge.bin");
        let mut record = sample_record(&path, &rel);
        record.state = "skipped";
        record.skip_reason = Some("too_large");

        upsert(&mut conn, &record, &[], &[]).unwrap();

        let row = get_by_path(&conn, &path).unwrap().unwrap();
        assert_eq!(row.state, "skipped");
        assert_eq!(row.skip_reason.as_deref(), Some("too_large"));
    }

    #[test]
    fn chunk_source_is_stored_verbatim() {
        let (_dir, mut conn) = open_test_db();
        let path = PathBuf::from("/roots/a/notes.txt");
        let rel = PathBuf::from("notes.txt");
        let chunk = RawChunk {
            source: ChunkSource::Filename,
            text: "notes txt".to_string(),
            page: None,
            line_start: None,
            line_end: None,
        };

        let chunks = [chunk];
        upsert(
            &mut conn,
            &sample_record(&path, &rel),
            &chunks,
            &fake_embeddings(&chunks),
        )
        .unwrap();

        let source: String = conn
            .query_row("SELECT source FROM chunks LIMIT 1", [], |row| row.get(0))
            .unwrap();
        assert_eq!(source, "filename");
    }

    #[test]
    fn upsert_file_writes_one_vec_text_row_per_chunk() {
        let (_dir, mut conn) = open_test_db();
        let path = PathBuf::from("/roots/a/notes.txt");
        let rel = PathBuf::from("notes.txt");
        let chunks = vec![
            RawChunk::body("apple banana".to_string()),
            RawChunk::body("cherry date".to_string()),
        ];
        let embeddings = fake_embeddings(&chunks);

        upsert(&mut conn, &sample_record(&path, &rel), &chunks, &embeddings).unwrap();

        let vec_rows: i64 = conn
            .query_row("SELECT COUNT(*) FROM vec_text", [], |row| row.get(0))
            .unwrap();
        assert_eq!(vec_rows, 2);
    }

    #[test]
    fn reindexing_replaces_vec_text_rows_without_duplicating() {
        let (_dir, mut conn) = open_test_db();
        let path = PathBuf::from("/roots/a/notes.txt");
        let rel = PathBuf::from("notes.txt");

        let first_chunks = vec![RawChunk::body("version one".to_string())];
        upsert(
            &mut conn,
            &sample_record(&path, &rel),
            &first_chunks,
            &fake_embeddings(&first_chunks),
        )
        .unwrap();

        let second_chunks = vec![
            RawChunk::body("version two".to_string()),
            RawChunk::body("more text".to_string()),
        ];
        upsert(
            &mut conn,
            &sample_record(&path, &rel),
            &second_chunks,
            &fake_embeddings(&second_chunks),
        )
        .unwrap();

        let vec_rows: i64 = conn
            .query_row("SELECT COUNT(*) FROM vec_text", [], |row| row.get(0))
            .unwrap();
        assert_eq!(
            vec_rows, 2,
            "stale vectors from the first version must be gone"
        );
    }

    #[test]
    fn fewer_embeddings_than_chunks_is_an_error_not_a_silent_truncation() {
        let (_dir, mut conn) = open_test_db();
        let path = PathBuf::from("/roots/a/notes.txt");
        let rel = PathBuf::from("notes.txt");
        let chunks = vec![
            RawChunk::body("one".to_string()),
            RawChunk::body("two".to_string()),
        ];

        let err = upsert(
            &mut conn,
            &sample_record(&path, &rel),
            &chunks,
            &fake_embeddings(&chunks[..1]),
        )
        .unwrap_err();

        assert!(matches!(err, crate::error::Error::Model(_)), "got {err:?}");
        assert_eq!(count_chunks(&conn).unwrap(), 0, "nothing partially written");
    }

    #[test]
    fn hash_key_and_image_vector_round_trip_and_are_replaced_on_reindex() {
        let (_dir, mut conn) = open_test_db();
        let path = PathBuf::from("/roots/a/photo.jpg");
        let rel = PathBuf::from("photo.jpg");
        let mut record = sample_record(&path, &rel);
        let hash = [7u8; 32];
        record.content_hash = Some(&hash);
        record.thumb_key = Some("abc");

        upsert_file(&mut conn, &record, &[], &[], Some(&[0.5; 768])).unwrap();
        upsert_file(&mut conn, &record, &[], &[], Some(&[0.25; 768])).unwrap();

        let (stored, key): (Vec<u8>, String) = conn
            .query_row("SELECT content_hash, thumb_key FROM files", [], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .unwrap();
        assert_eq!((stored, key.as_str()), (hash.to_vec(), "abc"));
        let vectors: i64 = conn
            .query_row("SELECT COUNT(*) FROM vec_image", [], |r| r.get(0))
            .unwrap();
        assert_eq!(vectors, 1);

        upsert_file(&mut conn, &record, &[], &[], None).unwrap();
        let vectors: i64 = conn
            .query_row("SELECT COUNT(*) FROM vec_image", [], |r| r.get(0))
            .unwrap();
        assert_eq!(vectors, 0);
    }

    // ---- M5 slice 1: state machine, deletion, purge ----

    fn add_file(conn: &mut Connection, root_id: i64, name: &str, mtime_ns: i64, body: &str) -> i64 {
        let path = PathBuf::from(format!("/roots/{root_id}/{name}"));
        let rel = PathBuf::from(name);
        let mut record = sample_record(&path, &rel);
        record.root_id = root_id;
        record.file_name = name;
        record.mtime_ns = mtime_ns;
        let chunks = vec![RawChunk::body(body.to_string())];
        upsert_file(
            conn,
            &record,
            &chunks,
            &fake_embeddings(&chunks),
            Some(&[0.5; 768]),
        )
        .unwrap()
    }

    fn scalar(conn: &Connection, sql: &str) -> i64 {
        conn.query_row(sql, [], |r| r.get(0)).unwrap()
    }

    /// SPEC.md §7 M5 item 7: gone from files, chunks, chunks_fts, vec_text and
    /// vec_image.
    #[test]
    fn deleting_a_file_removes_every_trace_of_it() {
        let (_dir, mut conn) = open_test_db();
        let keep = add_file(&mut conn, 1, "keep.txt", 1, "kept words");
        let gone = add_file(&mut conn, 1, "gone.txt", 2, "vanishing words");

        assert!(delete_file(&mut conn, gone).unwrap());

        assert_eq!(scalar(&conn, "SELECT COUNT(*) FROM files"), 1);
        assert_eq!(scalar(&conn, "SELECT COUNT(*) FROM chunks"), 1);
        assert_eq!(scalar(&conn, "SELECT COUNT(*) FROM vec_text"), 1);
        assert_eq!(scalar(&conn, "SELECT COUNT(*) FROM vec_image"), 1);
        assert_eq!(
            scalar(
                &conn,
                "SELECT COUNT(*) FROM chunks_fts WHERE chunks_fts MATCH 'vanishing'"
            ),
            0
        );
        assert_eq!(
            scalar(
                &conn,
                "SELECT COUNT(*) FROM chunks_fts WHERE chunks_fts MATCH 'kept'"
            ),
            1
        );
        assert_eq!(scalar(&conn, "SELECT id FROM files"), keep);
        assert!(!delete_file(&mut conn, gone).unwrap(), "already gone");
    }

    #[test]
    fn purging_a_root_removes_only_its_files() {
        let (_dir, mut conn) = open_test_db();
        let other_dir = tempfile::tempdir().unwrap();
        let other = db::roots::add(&conn, other_dir.path()).unwrap().id;
        add_file(&mut conn, 1, "a.txt", 1, "first root");
        add_file(&mut conn, 1, "b.txt", 2, "first root too");
        add_file(&mut conn, other, "c.txt", 3, "second root");

        let tx = conn.transaction().unwrap();
        purge_root(&tx, 1).unwrap();
        tx.commit().unwrap();

        assert_eq!(scalar(&conn, "SELECT COUNT(*) FROM files"), 1);
        assert_eq!(scalar(&conn, "SELECT root_id FROM files"), other);
        assert_eq!(scalar(&conn, "SELECT COUNT(*) FROM vec_text"), 1);
        assert_eq!(scalar(&conn, "SELECT COUNT(*) FROM vec_image"), 1);
    }

    #[test]
    fn next_pending_is_newest_first_and_waits_out_a_retry_delay() {
        let (_dir, mut conn) = open_test_db();
        let old = add_file(&mut conn, 1, "old.txt", 10, "x");
        let new = add_file(&mut conn, 1, "new.txt", 30, "x");
        let waiting = add_file(&mut conn, 1, "waiting.txt", 20, "x");
        let done = add_file(&mut conn, 1, "done.txt", 40, "x");
        for id in [old, new, waiting] {
            mark_pending(&conn, id).unwrap();
        }
        conn.execute(
            "UPDATE files SET next_attempt_at = 1000 WHERE id = ?1",
            params![waiting],
        )
        .unwrap();

        let ids = |now| -> Vec<i64> {
            next_pending(&conn, now, 10)
                .unwrap()
                .iter()
                .map(|f| f.id)
                .collect()
        };

        assert_eq!(
            ids(999),
            vec![new, old],
            "not the waiting one, not the settled one"
        );
        assert_eq!(ids(1000), vec![new, waiting, old], "by mtime, newest first");
        assert!(!ids(1000).contains(&done));
        assert_eq!(
            next_pending(&conn, 1000, 1).unwrap().len(),
            1,
            "limit respected"
        );
    }

    #[test]
    fn a_failing_file_backs_off_then_goes_to_error() {
        let (_dir, mut conn) = open_test_db();
        let id = add_file(&mut conn, 1, "flaky.txt", 1, "x");
        mark_indexing(&conn, id).unwrap();

        assert_eq!(
            record_failure(&conn, id, "locked", 1000).unwrap(),
            Failure::Retry {
                attempts: 1,
                next_attempt_at: 1030
            }
        );
        assert_eq!(
            record_failure(&conn, id, "locked", 2000).unwrap(),
            Failure::Retry {
                attempts: 2,
                next_attempt_at: 2120
            }
        );
        let state = |conn: &Connection| -> String {
            conn.query_row("SELECT state FROM files", [], |r| r.get(0))
                .unwrap()
        };
        assert_eq!(state(&conn), "pending");

        assert_eq!(
            record_failure(&conn, id, "still locked", 3000).unwrap(),
            Failure::GaveUp { attempts: 3 }
        );
        assert_eq!(state(&conn), "error");
        let (message, next): (String, Option<i64>) = conn
            .query_row("SELECT error, next_attempt_at FROM files", [], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .unwrap();
        assert_eq!((message.as_str(), next), ("still locked", None));
        assert_eq!(
            (
                backoff_secs(1),
                backoff_secs(2),
                backoff_secs(3),
                backoff_secs(9)
            ),
            (30, 120, 600, 600)
        );
    }

    #[test]
    fn a_crashed_run_leaves_indexing_rows_that_reset_to_pending() {
        let (_dir, mut conn) = open_test_db();
        let a = add_file(&mut conn, 1, "a.txt", 1, "x");
        let b = add_file(&mut conn, 1, "b.txt", 2, "x");
        mark_indexing(&conn, a).unwrap();

        assert_eq!(reset_indexing_to_pending(&conn).unwrap(), 1);

        let state_of = |id: i64| -> String {
            conn.query_row("SELECT state FROM files WHERE id = ?1", params![id], |r| {
                r.get(0)
            })
            .unwrap()
        };
        assert_eq!(
            (state_of(a).as_str(), state_of(b).as_str()),
            ("pending", "indexed")
        );
    }

    #[test]
    fn invalidating_marks_files_stale_so_the_hash_skip_cannot_keep_them() {
        let (_dir, mut conn) = open_test_db();
        let text = add_file(&mut conn, 1, "a.txt", 1, "x");
        let photo = {
            let path = PathBuf::from("/roots/1/p.jpg");
            let rel = PathBuf::from("p.jpg");
            let mut record = sample_record(&path, &rel);
            record.kind = "image";
            upsert_file(&mut conn, &record, &[], &[], None).unwrap()
        };

        assert_eq!(invalidate(&conn, Some("image")).unwrap(), 1);
        let row = |id: i64| -> (String, i64) {
            conn.query_row(
                "SELECT state, pipeline_version FROM files WHERE id = ?1",
                params![id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap()
        };
        assert_eq!(row(photo), ("pending".to_string(), 0));
        assert_eq!(row(text).0, "indexed");

        assert_eq!(invalidate(&conn, None).unwrap(), 2);
        assert_eq!(row(text), ("pending".to_string(), 0));
    }
}
