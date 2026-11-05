//! The work queue: state changes, retries with backoff, and counts.

use std::path::PathBuf;

use rusqlite::{Connection, params};

use crate::db::roots::indexable_sql;
use crate::dto::{FileError, FileErrorCode};
use crate::error::Result;

use super::FileState;

/// Attempts before a file goes to `error` (SPEC.md §5.4).
pub const MAX_ATTEMPTS: u32 = 3;

/// Delay before the next try after the `attempt`-th failure: 30 s, 2 min,
/// then 10 min (SPEC.md §5.4 step 6). The last value repeats.
pub fn backoff_secs(attempt: u32) -> i64 {
    const SECS: [i64; 3] = [30, 120, 600];
    SECS[(attempt.saturating_sub(1) as usize).min(SECS.len() - 1)]
}

/// Records that a file was looked at and its indexed content is still right:
/// only `size`, `mtime_ns` and its `state` change, so
/// nothing is re-extracted or re-embedded (SPEC.md §5.4 step 4).
pub fn touch_unchanged(
    conn: &Connection,
    file_id: i64,
    size: u64,
    mtime_ns: i64,
    state: FileState,
) -> Result<()> {
    conn.execute(
        "UPDATE files SET size = ?2, mtime_ns = ?3, state = ?4,
                attempts = 0, next_attempt_at = NULL
         WHERE id = ?1",
        params![file_id, size as i64, mtime_ns, state],
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

// `INDEXED BY`: left to itself the planner takes `idx_files_state` and sorts
// every pending row per call (checked with EXPLAIN on a fresh database).
pub(super) const NEXT_PENDING_SQL: &str = concat!(
    "SELECT id, root_id, path, size, mtime_ns
     FROM files INDEXED BY idx_files_pending
     WHERE state = 'pending' AND (next_attempt_at IS NULL OR next_attempt_at <= ?1)
       AND root_id IN (SELECT id FROM roots WHERE ",
    indexable_sql!(),
    ")
     ORDER BY mtime_ns DESC, id
     LIMIT ?2"
);

/// The next files to work on: `pending`, not waiting out a retry delay, in a
/// root that is enabled and readable, most recently modified first so fresh
/// files become searchable first (SPEC.md §5.4 step 6). Read in order from the
/// partial `idx_files_pending` (migration 0002), not sorted per call.
pub fn next_pending(conn: &Connection, now: i64, limit: u32) -> Result<Vec<PendingFile>> {
    let mut stmt = conn.prepare_cached(NEXT_PENDING_SQL)?;
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

pub fn record_failure(
    conn: &Connection,
    file_id: i64,
    code: FileErrorCode,
    message: &str,
    now: i64,
) -> Result<Failure> {
    let attempts = attempts_of(conn, file_id)? + 1;
    if attempts >= MAX_ATTEMPTS {
        conn.execute(
            "UPDATE files SET state = 'error', attempts = ?2, error = ?3, error_code = ?4,
                    next_attempt_at = NULL
             WHERE id = ?1",
            params![file_id, attempts, message, code],
        )?;
        return Ok(Failure::GaveUp { attempts });
    }
    retry_later(conn, file_id, code, attempts, message, now)
}

/// A file another program holds open (Windows sharing or lock violation):
/// always retried, never `error` (SPEC.md §5.4 step 6, §6.2). The count grows
/// so the backoff does, but stops one short of [`MAX_ATTEMPTS`].
///
/// ponytail: shares `attempts` with real failures, so one real failure right
/// after a long lock gives up at once; a separate lock counter if that bites.
pub fn record_locked(
    conn: &Connection,
    file_id: i64,
    code: FileErrorCode,
    message: &str,
    now: i64,
) -> Result<Failure> {
    let attempts = (attempts_of(conn, file_id)? + 1).min(MAX_ATTEMPTS - 1);
    retry_later(conn, file_id, code, attempts, message, now)
}

fn attempts_of(conn: &Connection, file_id: i64) -> Result<u32> {
    Ok(conn.query_row(
        "SELECT attempts FROM files WHERE id = ?1",
        params![file_id],
        |row| row.get::<_, u32>(0),
    )?)
}

fn retry_later(
    conn: &Connection,
    file_id: i64,
    code: FileErrorCode,
    attempts: u32,
    message: &str,
    now: i64,
) -> Result<Failure> {
    let next_attempt_at = now + backoff_secs(attempts);
    conn.execute(
        "UPDATE files SET state = 'pending', attempts = ?2, error = ?3, error_code = ?5,
                next_attempt_at = ?4
         WHERE id = ?1",
        params![file_id, attempts, message, next_attempt_at, code],
    )?;
    Ok(Failure::Retry {
        attempts,
        next_attempt_at,
    })
}

/// Puts every `error` file back in the queue with attempts and backoff cleared
/// (SPEC.md §5.7 `retry_errors`). The error stays until a result replaces it,
/// so the file is read again rather than kept by its hash. Returns how many.
pub fn retry_errors(conn: &Connection) -> Result<usize> {
    Ok(conn.execute(
        "UPDATE files SET state = 'pending', attempts = 0, next_attempt_at = NULL
         WHERE state = 'error'",
        [],
    )?)
}

/// Files in `error`, newest first (SPEC.md §5.7 `list_errors`).
pub fn list_errors(conn: &Connection, limit: u32) -> Result<Vec<FileError>> {
    let mut stmt = conn.prepare_cached(
        "SELECT id, path, COALESCE(error_code, 'other'), COALESCE(error, ''), attempts
         FROM files WHERE state = 'error' ORDER BY id DESC LIMIT ?1",
    )?;
    let rows = stmt
        .query_map(params![limit], |row| {
            Ok(FileError {
                file_id: row.get(0)?,
                path: row.get(1)?,
                code: row.get(2)?,
                detail: row.get(3)?,
                attempts: row.get(4)?,
            })
        })?
        .collect::<std::result::Result<_, _>>()?;
    Ok(rows)
}

/// How many files are in each state.
pub fn count_states(conn: &Connection) -> Result<std::collections::HashMap<FileState, u64>> {
    // `pending` rows of a root that is off or unreadable wait for nothing the
    // scheduler will start (see `next_pending`), so they are not queue.
    let mut stmt = conn.prepare_cached(concat!(
        "SELECT state, COUNT(*) FROM files
         WHERE state <> 'pending' OR root_id IN (SELECT id FROM roots WHERE ",
        indexable_sql!(),
        ") GROUP BY state"
    ))?;
    let counts = stmt
        .query_map([], |row| Ok((row.get(0)?, row.get::<_, i64>(1)? as u64)))?
        .collect::<std::result::Result<_, _>>()?;
    Ok(counts)
}

/// Indexed files per root id; a root with none is absent.
pub fn count_indexed_by_root(conn: &Connection) -> Result<std::collections::HashMap<i64, u64>> {
    let mut stmt = conn
        .prepare_cached("SELECT root_id, COUNT(*) FROM files WHERE state = ?1 GROUP BY root_id")?;
    let counts = stmt
        .query_map([FileState::Indexed], |row| {
            Ok((row.get(0)?, row.get::<_, i64>(1)? as u64))
        })?
        .collect::<std::result::Result<_, _>>()?;
    Ok(counts)
}

pub fn count_files(conn: &Connection) -> Result<i64> {
    Ok(conn.query_row("SELECT COUNT(*) FROM files", [], |row| row.get(0))?)
}

pub fn count_chunks(conn: &Connection) -> Result<i64> {
    Ok(conn.query_row("SELECT COUNT(*) FROM chunks", [], |row| row.get(0))?)
}
