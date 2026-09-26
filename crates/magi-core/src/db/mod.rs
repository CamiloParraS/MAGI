//! Connection factory (WAL, `foreign_keys`, `busy_timeout`), sqlite-vec
//! registration, and the migration runner. Implemented in M1 (see SPEC.md
//! §7 M1, §5.5 schema).

pub mod files;
pub mod meta;
pub mod roots;

use std::path::Path;
use std::sync::Once;

use rusqlite::Connection;

use crate::error::Result;

/// One embedded migration: schema version and its SQL.
const MIGRATIONS: &[(i64, &str)] = &[
    (1, include_str!("migrations/0001_init.sql")),
    (2, include_str!("migrations/0002_files_indexes.sql")),
    (3, include_str!("migrations/0003_features_missing.sql")),
    (4, include_str!("migrations/0004_error_code.sql")),
];

static VEC_EXTENSION_REGISTERED: Once = Once::new();

type SqliteExtensionInit = unsafe extern "C" fn(
    *mut rusqlite::ffi::sqlite3,
    *mut *mut std::os::raw::c_char,
    *const rusqlite::ffi::sqlite3_api_routines,
) -> std::os::raw::c_int;

fn register_vec_extension() {
    VEC_EXTENSION_REGISTERED.call_once(|| unsafe {
        rusqlite::ffi::sqlite3_auto_extension(Some(std::mem::transmute::<
            *const (),
            SqliteExtensionInit,
        >(
            sqlite_vec::sqlite3_vec_init as *const ()
        )));
    });
}

/// Opens (creating if needed) the database at `path`, applies pragmas, and
/// runs any pending migrations.
pub fn open(path: &Path) -> Result<Connection> {
    register_vec_extension();
    let conn = Connection::open(path)?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    // No fsync per commit (the writer commits per file). A power loss can
    // drop the last commits but not corrupt the database; the files they
    // covered are left `indexing`/`pending` and redone (SPEC.md §5.4).
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    conn.busy_timeout(std::time::Duration::from_secs(5))?;
    run_migrations(&conn)?;
    Ok(conn)
}

/// Applies every migration with a version greater than the one already
/// recorded. Safe to call repeatedly: already-applied migrations are
/// skipped, so running it twice on the same database is a no-op the second
/// time.
pub fn run_migrations(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_migrations (
            version INTEGER PRIMARY KEY,
            applied_at INTEGER NOT NULL
        )",
    )?;

    let current: i64 = conn.query_row(
        "SELECT COALESCE(MAX(version), 0) FROM schema_migrations",
        [],
        |row| row.get(0),
    )?;

    for (version, sql) in MIGRATIONS {
        if *version <= current {
            continue;
        }
        conn.execute_batch(sql)?;
        conn.execute(
            "INSERT INTO schema_migrations (version, applied_at) VALUES (?1, unixepoch())",
            [*version],
        )?;
    }
    Ok(())
}

/// `sqlite_version()`, for `magi-cli doctor`.
pub fn sqlite_version(conn: &Connection) -> Result<String> {
    Ok(conn.query_row("SELECT sqlite_version()", [], |row| row.get(0))?)
}

/// Whether FTS5 was compiled in, for `magi-cli doctor`.
pub fn fts5_enabled(conn: &Connection) -> Result<bool> {
    let flag: i64 = conn.query_row(
        "SELECT sqlite_compileoption_used('ENABLE_FTS5')",
        [],
        |row| row.get(0),
    )?;
    Ok(flag == 1)
}

/// `vec_version()`, for `magi-cli doctor`.
pub fn vec_version(conn: &Connection) -> Result<String> {
    Ok(conn.query_row("SELECT vec_version()", [], |row| row.get(0))?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrations_are_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let conn = open(&dir.path().join("magi.db")).unwrap();
        // open() already ran migrations once; running again must not error
        // and must not reapply anything.
        run_migrations(&conn).unwrap();

        let applied: i64 = conn
            .query_row("SELECT COUNT(*) FROM schema_migrations", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(applied, MIGRATIONS.len() as i64);
    }

    /// WAL + NORMAL: no fsync per commit, still corruption-safe.
    #[test]
    fn commits_are_not_fsynced_one_by_one() {
        let dir = tempfile::tempdir().unwrap();
        let conn = open(&dir.path().join("magi.db")).unwrap();
        let synchronous: i64 = conn
            .pragma_query_value(None, "synchronous", |r| r.get(0))
            .unwrap();
        assert_eq!(synchronous, 1, "NORMAL");
    }

    #[test]
    fn schema_has_fts5_and_vec() {
        let dir = tempfile::tempdir().unwrap();
        let conn = open(&dir.path().join("magi.db")).unwrap();
        assert!(fts5_enabled(&conn).unwrap());
        assert!(vec_version(&conn).unwrap().starts_with('v'));
        assert!(!sqlite_version(&conn).unwrap().is_empty());
    }
}
