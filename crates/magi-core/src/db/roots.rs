//! Root repository, including nested-root rejection. Implemented in M1
//! (see SPEC.md §7 M1).

use std::path::{Path, PathBuf};

use rusqlite::{Connection, OptionalExtension, params};

use crate::error::{Error, Result};
use crate::paths;
use crate::platform::RootAccess;

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

pub fn get(conn: &Connection, id: i64) -> Result<Root> {
    list(conn)?
        .into_iter()
        .find(|r| r.id == id)
        .ok_or(Error::RootIdNotFound(id))
}

/// A disabled root keeps its rows but is not indexed, watched or searched.
pub fn set_enabled(conn: &Connection, id: i64, enabled: bool) -> Result<()> {
    let affected = conn.execute(
        "UPDATE roots SET enabled = ?2 WHERE id = ?1",
        params![id, enabled],
    )?;
    if affected == 0 {
        return Err(Error::RootIdNotFound(id));
    }
    Ok(())
}

/// Removes a root and everything indexed under it: files, chunks, FTS rows and
/// both vector tables, in one transaction (SPEC.md §7 M5 item 12). Without the
/// purge the delete would fail on the `files.root_id` foreign key.
pub fn remove(conn: &Connection, id: i64) -> Result<()> {
    let tx = conn.unchecked_transaction()?;
    let thumbnail_keys = super::files::purge_root(&tx, id)?;
    let affected = tx.execute("DELETE FROM roots WHERE id = ?1", params![id])?;
    if affected == 0 {
        return Err(Error::RootIdNotFound(id));
    }
    tx.commit()?;
    super::files::remove_unreferenced_thumbnails(conn, &thumbnail_keys);
    Ok(())
}

/// Sets a root's status directly (e.g. `watch_failed`).
pub fn set_status(conn: &Connection, id: i64, status: &str) -> Result<()> {
    conn.execute(
        "UPDATE roots SET status = ?2 WHERE id = ?1",
        params![id, status],
    )?;
    Ok(())
}

/// Records what the access probe found. `ok` only replaces the statuses a probe
/// can set, so it does not clear `watch_failed`.
pub fn set_access(conn: &Connection, id: i64, access: &RootAccess) -> Result<()> {
    let status = match access {
        RootAccess::Ok => "ok",
        RootAccess::PermissionDenied => "permission_denied",
        RootAccess::Missing => "missing",
    };
    conn.execute(
        "UPDATE roots SET status = ?2
         WHERE id = ?1 AND (?2 <> 'ok' OR status IN ('missing', 'permission_denied'))",
        params![id, status],
    )?;
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
    fn a_root_can_be_disabled_enabled_and_read_back() {
        let (_dir, conn) = open_test_db();
        let root_dir = tempfile::tempdir().unwrap();
        let root = add(&conn, root_dir.path()).unwrap();

        set_enabled(&conn, root.id, false).unwrap();
        assert!(!get(&conn, root.id).unwrap().enabled);
        set_enabled(&conn, root.id, true).unwrap();
        assert_eq!(get(&conn, root.id).unwrap(), root);
        assert!(matches!(
            set_enabled(&conn, root.id + 1, true),
            Err(Error::RootIdNotFound(_))
        ));
        assert!(matches!(
            get(&conn, root.id + 1),
            Err(Error::RootIdNotFound(_))
        ));
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

    /// SPEC.md §7 M5 item 12: removing a root purges everything under it. This
    /// used to fail outright on the `files.root_id` foreign key.
    #[test]
    fn removing_a_root_purges_its_files_chunks_and_vectors() {
        use crate::db::files::{FileRecord, upsert_file};
        use crate::embed::{FakeEmbedder, TextEmbedder};
        use crate::extract::RawChunk;

        let dir = tempfile::tempdir().unwrap();
        let mut conn = db::open(&dir.path().join("magi.db")).unwrap();
        let root_dir = tempfile::tempdir().unwrap();
        let root = add(&conn, root_dir.path()).unwrap();
        let path = root.path.join("notes.txt");
        let rel = std::path::PathBuf::from("notes.txt");
        let chunks = vec![RawChunk::body("hello world".to_string())];
        let embeddings = FakeEmbedder.embed_passages(&["hello world"]).unwrap();
        let record = FileRecord {
            root_id: root.id,
            path: &path,
            rel_path: &rel,
            file_name: "notes.txt",
            ext: Some("txt"),
            kind: "text",
            size: 11,
            mtime_ns: 1,
            lang: None,
            state: "indexed",
            skip_reason: None,
            error: None,
            seen_scan_id: 1,
            content_hash: None,
            thumb_key: None,
        };
        upsert_file(&mut conn, &record, &chunks, &embeddings, Some(&[0.5; 768])).unwrap();

        remove(&conn, root.id).unwrap();

        let count = |table: &str| -> i64 {
            conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
                .unwrap()
        };
        for table in ["roots", "files", "chunks", "vec_text", "vec_image"] {
            assert_eq!(count(table), 0, "{table} must be empty");
        }
        assert_eq!(
            count("chunks_fts WHERE chunks_fts MATCH 'hello'"),
            0,
            "and nothing searchable is left"
        );
    }
}
