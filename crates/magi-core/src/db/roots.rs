//! Root repository, including nested-root rejection and collapsing child
//! roots into a newly added parent.

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
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
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
mod tests;
