//! Root repository, including nested-root rejection and collapsing child
//! roots into a newly added parent. Implemented in M1
//! (see SPEC.md §7 M1).

use std::path::{Path, PathBuf};

use rusqlite::types::{FromSql, FromSqlError, FromSqlResult, ToSql, ToSqlOutput, ValueRef};
use rusqlite::{Connection, OptionalExtension, params};

use crate::error::{Error, Result};
use crate::paths;
use crate::platform::RootAccess;

#[derive(Debug, Clone, PartialEq)]
pub struct Root {
    pub id: i64,
    pub path: PathBuf,
    pub enabled: bool,
    pub status: Health,
}

impl Root {
    /// Scanned, queued and indexed: enabled and readable.
    pub fn indexable(&self) -> bool {
        self.enabled && self.status.readable()
    }

    /// Shown in search: enabled and not missing. An unreadable root's index is
    /// still right, so it stays searchable; a missing one is hidden until it is
    /// back.
    pub fn searchable(&self) -> bool {
        self.enabled && self.status != Health::Missing
    }
}

/// What a root's last probe or watch attempt found (`roots.status`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Health {
    Ok,
    PermissionDenied,
    Missing,
    /// Readable but not watched: polled instead.
    WatchFailed,
}

impl Health {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::PermissionDenied => "permission_denied",
            Self::Missing => "missing",
            Self::WatchFailed => "watch_failed",
        }
    }

    /// Its files can be read, so it is scanned, watched or polled, and indexed.
    /// The other statuses are re-probed until the root is back.
    pub fn readable(self) -> bool {
        matches!(self, Self::Ok | Self::WatchFailed)
    }
}

impl From<RootAccess> for Health {
    fn from(access: RootAccess) -> Self {
        match access {
            RootAccess::Ok => Self::Ok,
            RootAccess::PermissionDenied => Self::PermissionDenied,
            RootAccess::Missing => Self::Missing,
        }
    }
}

impl std::fmt::Display for Health {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl ToSql for Health {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        Ok(self.as_str().into())
    }
}

impl FromSql for Health {
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        match value.as_str()? {
            "ok" => Ok(Self::Ok),
            "permission_denied" => Ok(Self::PermissionDenied),
            "missing" => Ok(Self::Missing),
            "watch_failed" => Ok(Self::WatchFailed),
            other => Err(FromSqlError::Other(
                format!("unknown root status {other:?}").into(),
            )),
        }
    }
}

// The same rules as SQL over a `roots` row, for queries to splice in with
// `concat!`. Column names are bare, so they work with or without a `roots r`
// alias; only `roots` has `enabled` and `status`. `root_rules_agree_with_their_sql`
// checks each against its Rust twin.

/// [`Health::readable`].
macro_rules! readable_sql {
    () => {
        "status IN ('ok', 'watch_failed')"
    };
}
/// [`Root::indexable`].
macro_rules! indexable_sql {
    () => {
        concat!("enabled = 1 AND ", $crate::db::roots::readable_sql!())
    };
}
/// [`Root::searchable`].
macro_rules! searchable_sql {
    () => {
        "enabled = 1 AND status <> 'missing'"
    };
}
pub(crate) use {indexable_sql, readable_sql, searchable_sql};

/// [`add_collapsing`] for callers that don't care which roots it collapsed.
pub fn add(conn: &Connection, path: &Path) -> Result<Root> {
    add_collapsing(conn, path).map(|(root, _)| root)
}

/// Registers a root after canonicalizing it. Rejects a path that is missing,
/// already registered, or inside an existing root (already covered). Existing
/// roots inside the new one are collapsed into it: their files are re-pointed
/// (root, relative path, filename chunk) so the next scan finds them unchanged
/// instead of re-indexing them, and their rows are deleted. Returns the new
/// root and the ids of the collapsed ones.
pub fn add_collapsing(conn: &Connection, path: &Path) -> Result<(Root, Vec<i64>)> {
    if !path.exists() {
        return Err(Error::RootNotFound(path.to_path_buf()));
    }
    let canonical = paths::canonicalize(path)?;

    let mut children = Vec::new();
    for existing in list(conn)? {
        if existing.path == canonical {
            return Err(Error::RootAlreadyExists(canonical));
        }
        if canonical.starts_with(&existing.path) {
            return Err(Error::NestedRoot {
                path: canonical,
                conflicts_with: existing.path,
            });
        }
        if existing.path.starts_with(&canonical) {
            children.push(existing.id);
        }
    }

    let tx = conn.unchecked_transaction()?;
    tx.execute(
        "INSERT INTO roots (path, enabled, status, added_at)
         VALUES (?1, 1, 'ok', unixepoch())",
        params![canonical.to_string_lossy()],
    )?;
    let id = tx.last_insert_rowid();
    for &child in &children {
        // In-flight results carry the child's id; `pending` makes the writer drop them.
        tx.execute(
            "UPDATE files SET state = 'pending' WHERE root_id = ?1 AND state = 'indexing'",
            params![child],
        )?;
        let rows = tx
            .prepare("SELECT id, path, size, mtime_ns, seen_scan_id FROM files WHERE root_id = ?1")?
            .query_map(params![child], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    PathBuf::from(row.get::<_, String>(1)?),
                    row.get::<_, i64>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, i64>(4)?,
                ))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        // ponytail: one UPDATE per file; bulk SQL if collapsing huge roots is slow.
        for (file_id, file_path, size, mtime_ns, seen) in rows {
            let rel = file_path.strip_prefix(&canonical).unwrap_or(&file_path);
            let to = super::files::MoveTarget {
                path: &file_path,
                rel_path: rel,
                size: size as u64,
                mtime_ns,
            };
            super::files::rename_file_to(&tx, file_id, id, to, seen)?;
        }
        tx.execute("DELETE FROM roots WHERE id = ?1", params![child])?;
    }
    tx.commit()?;

    let root = Root {
        id,
        path: canonical,
        enabled: true,
        status: Health::Ok,
    };
    Ok((root, children))
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
pub fn set_status(conn: &Connection, id: i64, status: Health) -> Result<()> {
    conn.execute(
        "UPDATE roots SET status = ?2 WHERE id = ?1",
        params![id, status],
    )?;
    Ok(())
}

/// Records what the access probe found. `ok` only replaces an unreadable
/// status, so it does not clear `watch_failed`.
pub fn set_access(conn: &Connection, id: i64, access: &RootAccess) -> Result<()> {
    conn.execute(
        concat!(
            "UPDATE roots SET status = ?2
             WHERE id = ?1 AND (?2 <> 'ok' OR NOT ",
            readable_sql!(),
            ")"
        ),
        params![id, Health::from(*access)],
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

    /// Every status reads back as written, and each rule gives the same answer
    /// in Rust and in the SQL the queries splice in.
    #[test]
    fn root_rules_agree_with_their_sql() {
        let (_dir, conn) = open_test_db();
        let root_dir = tempfile::tempdir().unwrap();
        let id = add(&conn, root_dir.path()).unwrap().id;
        let sql = |rule: &str| -> bool {
            conn.query_row(
                &format!("SELECT COUNT(*) FROM roots WHERE id = ?1 AND {rule}"),
                [id],
                |r| r.get::<_, i64>(0),
            )
            .unwrap()
                == 1
        };
        let all = [
            Health::Ok,
            Health::PermissionDenied,
            Health::Missing,
            Health::WatchFailed,
        ];
        for health in all {
            for enabled in [true, false] {
                set_status(&conn, id, health).unwrap();
                set_enabled(&conn, id, enabled).unwrap();
                let root = get(&conn, id).unwrap();
                let case = format!("{health} enabled={enabled}");
                assert_eq!(root.status, health, "{case}");
                assert_eq!(root.indexable(), sql(indexable_sql!()), "{case}");
                assert_eq!(root.searchable(), sql(searchable_sql!()), "{case}");
                assert_eq!(health.readable(), sql(readable_sql!()), "{case}");
            }
        }
    }

    #[test]
    fn an_unreadable_root_stays_searchable_but_is_not_indexed() {
        let root = |status, enabled| Root {
            id: 1,
            path: PathBuf::new(),
            enabled,
            status,
        };
        assert!(
            root(Health::WatchFailed, true).indexable(),
            "polled instead"
        );
        assert!(!root(Health::PermissionDenied, true).indexable());
        assert!(root(Health::PermissionDenied, true).searchable());
        assert!(!root(Health::Missing, true).searchable());
        assert!(!root(Health::Ok, false).indexable());
        assert!(!root(Health::Ok, false).searchable());
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

    /// Adding a parent of an existing root takes the child's files over as
    /// they are: re-pointed, not re-indexed.
    #[test]
    fn adding_parent_collapses_child_and_keeps_its_files() {
        use crate::db::files::upsert_file;
        use crate::embed::{FakeEmbedder, TextEmbedder};
        use crate::extract::RawChunk;

        let dir = tempfile::tempdir().unwrap();
        let mut conn = db::open(&dir.path().join("magi.db")).unwrap();
        let parent = tempfile::tempdir().unwrap();
        let child_dir = parent.path().join("child");
        std::fs::create_dir(&child_dir).unwrap();
        let child = add(&conn, &child_dir).unwrap();
        let path = child.path.join("notes.txt");
        let rel = PathBuf::from("notes.txt");
        let chunks = vec![
            RawChunk::body("hello world".to_string()),
            crate::extract::filename::filename_chunk(&rel),
        ];
        let embeddings = FakeEmbedder
            .embed_passages(&["hello world", "notes"])
            .unwrap();
        upsert_file(
            &mut conn,
            &indexed_record(child.id, &path, &rel),
            &chunks,
            &embeddings,
            None,
        )
        .unwrap();

        let (root, collapsed) = add_collapsing(&conn, parent.path()).unwrap();

        assert_eq!(collapsed, vec![child.id]);
        assert_eq!(list(&conn).unwrap(), vec![root.clone()]);
        let (root_id, rel_path, state): (i64, String, String) = conn
            .query_row("SELECT root_id, rel_path, state FROM files", [], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            })
            .unwrap();
        assert_eq!(root_id, root.id);
        assert_eq!(
            PathBuf::from(rel_path),
            Path::new("child").join("notes.txt")
        );
        assert_eq!(state, "indexed");
        let count = |sql: &str| -> i64 { conn.query_row(sql, [], |r| r.get(0)).unwrap() };
        assert_eq!(count("SELECT COUNT(*) FROM vec_text"), 2);
        assert_eq!(
            count("SELECT COUNT(*) FROM chunks_fts WHERE chunks_fts MATCH 'child'"),
            1,
            "the filename chunk follows the new rel_path"
        );
    }

    /// A child file mid-pipeline goes back to `pending`, so the writer drops
    /// the in-flight result (stamped with the deleted child's id and rel path)
    /// instead of failing it on the foreign key and burning a retry.
    #[test]
    fn collapsing_requeues_child_files_mid_pipeline() {
        let dir = tempfile::tempdir().unwrap();
        let mut conn = db::open(&dir.path().join("magi.db")).unwrap();
        let parent = tempfile::tempdir().unwrap();
        let child_dir = parent.path().join("child");
        std::fs::create_dir(&child_dir).unwrap();
        let child = add(&conn, &child_dir).unwrap();
        let path = child.path.join("notes.txt");
        let rel = PathBuf::from("notes.txt");
        let mut record = indexed_record(child.id, &path, &rel);
        record.state = crate::db::files::FileState::Indexing;
        crate::db::files::upsert_file(&mut conn, &record, &[], &[], None).unwrap();

        add(&conn, parent.path()).unwrap();

        let state: String = conn
            .query_row("SELECT state FROM files", [], |r| r.get(0))
            .unwrap();
        assert_eq!(state, "pending");
    }

    fn indexed_record<'a>(
        root_id: i64,
        path: &'a Path,
        rel: &'a Path,
    ) -> crate::db::files::FileRecord<'a> {
        crate::db::files::FileRecord {
            root_id,
            path,
            rel_path: rel,
            file_name: "notes.txt",
            ext: Some("txt"),
            kind: "text",
            size: 11,
            mtime_ns: 1,
            lang: None,
            state: crate::db::files::FileState::Indexed,
            skip_reason: None,
            error: None,
            seen_scan_id: 1,
            content_hash: None,
            thumb_key: None,
        }
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
            state: crate::db::files::FileState::Indexed,
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
