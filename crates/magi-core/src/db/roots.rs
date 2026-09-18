//! Root repository, including nested-root rejection. Implemented in M1
//! (see SPEC.md §7 M1).

use std::path::{Path, PathBuf};

use rusqlite::{Connection, OptionalExtension, params};

use crate::error::{Error, Result};
use crate::paths;

#[derive(Debug, Clone, PartialEq)]
pub struct Root {
    pub id: i64,
    pub path: PathBuf,
    pub enabled: bool,
    pub status: String,
}

/// True if `path` is nested inside `other` (in either direction), not
/// counting the identical-path case — that's an "already exists" error,
/// not a "nested root" one (see `add`).
fn is_nested(path: &Path, other: &Path) -> bool {
    path != other && (path.starts_with(other) || other.starts_with(path))
}

/// Registers a root after canonicalizing it and rejecting it if it's
/// missing, already registered, or nested with an existing root.
pub fn add(conn: &Connection, path: &Path) -> Result<Root> {
    if !path.exists() {
        return Err(Error::RootNotFound(path.to_path_buf()));
    }
    let canonical = paths::canonicalize(path)?;

    for existing in list(conn)? {
        if is_nested(&canonical, &existing.path) {
            return Err(Error::NestedRoot {
                path: canonical,
                conflicts_with: existing.path,
            });
        }
    }

    let canonical_str = canonical.to_string_lossy().to_string();
    let inserted = conn
        .execute(
            "INSERT INTO roots (path, enabled, status, added_at)
             VALUES (?1, 1, 'ok', unixepoch())
             ON CONFLICT(path) DO NOTHING",
            params![canonical_str],
        )
        .map_err(Error::Db)?;
    if inserted == 0 {
        return Err(Error::RootAlreadyExists(canonical));
    }

    let id = conn.last_insert_rowid();
    Ok(Root {
        id,
        path: canonical,
        enabled: true,
        status: "ok".into(),
    })
}

pub fn list(conn: &Connection) -> Result<Vec<Root>> {
    let mut stmt = conn.prepare("SELECT id, path, enabled, status FROM roots ORDER BY id")?;
    let rows = stmt
        .query_map([], |row| {
            Ok(Root {
                id: row.get(0)?,
                path: PathBuf::from(row.get::<_, String>(1)?),
                enabled: row.get::<_, i64>(2)? != 0,
                status: row.get(3)?,
            })
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(rows)
}

pub fn remove(conn: &Connection, id: i64) -> Result<()> {
    let affected = conn.execute("DELETE FROM roots WHERE id = ?1", params![id])?;
    if affected == 0 {
        return Err(Error::RootIdNotFound(id));
    }
    Ok(())
}

#[allow(dead_code)]
fn find_by_path(conn: &Connection, path: &str) -> Result<Option<Root>> {
    conn.query_row(
        "SELECT id, path, enabled, status FROM roots WHERE path = ?1",
        params![path],
        |row| {
            Ok(Root {
                id: row.get(0)?,
                path: PathBuf::from(row.get::<_, String>(1)?),
                enabled: row.get::<_, i64>(2)? != 0,
                status: row.get(3)?,
            })
        },
    )
    .optional()
    .map_err(Error::Db)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    fn open_test_db() -> (tempfile::TempDir, Connection) {
        let dir = tempfile::tempdir().unwrap();
        let conn = db::open(&dir.path().join("magi.db")).unwrap();
        (dir, conn)
    }

    #[test]
    fn add_and_list_roundtrip() {
        let (_dir, conn) = open_test_db();
        let root_dir = tempfile::tempdir().unwrap();

        let added = add(&conn, root_dir.path()).unwrap();
        let roots = list(&conn).unwrap();

        assert_eq!(roots.len(), 1);
        assert_eq!(roots[0].id, added.id);
        assert_eq!(roots[0].path, added.path);
    }

    #[test]
    fn nested_root_is_rejected() {
        let (_dir, conn) = open_test_db();
        let parent = tempfile::tempdir().unwrap();
        let child = parent.path().join("child");
        std::fs::create_dir(&child).unwrap();

        add(&conn, parent.path()).unwrap();
        let err = add(&conn, &child).unwrap_err();
        assert!(matches!(err, Error::NestedRoot { .. }));
    }

    #[test]
    fn ancestor_of_existing_root_is_also_rejected() {
        let (_dir, conn) = open_test_db();
        let parent = tempfile::tempdir().unwrap();
        let child = parent.path().join("child");
        std::fs::create_dir(&child).unwrap();

        add(&conn, &child).unwrap();
        let err = add(&conn, parent.path()).unwrap_err();
        assert!(matches!(err, Error::NestedRoot { .. }));
    }

    #[test]
    fn readding_same_root_reports_already_exists_not_nested() {
        let (_dir, conn) = open_test_db();
        let root_dir = tempfile::tempdir().unwrap();

        add(&conn, root_dir.path()).unwrap();
        let err = add(&conn, root_dir.path()).unwrap_err();
        assert!(matches!(err, Error::RootAlreadyExists(_)));
    }

    #[test]
    fn missing_root_is_rejected() {
        let (_dir, conn) = open_test_db();
        let err = add(&conn, Path::new("/does/not/exist/hopefully")).unwrap_err();
        assert!(matches!(err, Error::RootNotFound(_)));
    }

    #[test]
    fn remove_unknown_id_errors() {
        let (_dir, conn) = open_test_db();
        assert!(matches!(
            remove(&conn, 999),
            Err(Error::RootIdNotFound(999))
        ));
    }
}
