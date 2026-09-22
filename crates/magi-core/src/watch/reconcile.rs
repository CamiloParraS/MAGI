//! Reconciliation scans: diff a walk of a root against its rows (SPEC.md §5.4
//! startup sequence step 4) and detect moves. Plain functions over a
//! connection; the runtime and the watcher call them.

use std::path::Path;

use rusqlite::Connection;

use crate::db::{files, meta, roots};
use crate::discovery;
use crate::error::Result;
use crate::index::PIPELINE_VERSION;
use crate::index::pipeline::{IndexRootOptions, hash_file};
use crate::platform::{FsProbe, PermissionProbe, RootAccess};

/// What a scan found. The `unseen` rows are deletion candidates, held for
/// [`resolve_moves`] rather than deleted here, so a move between roots can be
/// matched by scanning both roots first.
pub struct Reconciled {
    pub inserted: u32,
    pub changed: u32,
    pub unchanged: u32,
    pub unseen: Vec<files::Missing>,
}

/// The id for the next scan, one more than the last (`meta.last_scan_id`).
/// Rows carry the id of the scan that last saw them; an older one means gone.
pub fn next_scan_id(conn: &Connection) -> Result<i64> {
    let id = meta::get(conn, "last_scan_id")?
        .and_then(|v| v.parse::<i64>().ok())
        .unwrap_or(0)
        + 1;
    meta::set(conn, "last_scan_id", &id.to_string())?;
    Ok(id)
}

/// Walks `root_path` and brings its rows in line, in one transaction: an
/// unknown file is inserted `pending`; one whose size or mtime differ, or that
/// was indexed by an older pipeline, is marked `pending` with its new stat;
/// anything else is only marked seen. Nothing is extracted or deleted.
pub fn reconcile_root(
    conn: &mut Connection,
    root_id: i64,
    root_path: &Path,
    options: &IndexRootOptions,
    scan_id: i64,
) -> Result<Reconciled> {
    let entries = discovery::walk(root_path, &options.walk_options())?;
    let tx = conn.transaction()?;
    let (mut inserted, mut changed, mut unchanged) = (0, 0, 0);
    for entry in &entries {
        match files::get_stored(&tx, &entry.path)? {
            None => {
                let rel = entry.path.strip_prefix(root_path).unwrap_or(&entry.path);
                files::insert_pending(
                    &tx,
                    root_id,
                    &entry.path,
                    rel,
                    entry.size,
                    entry.mtime_ns,
                    scan_id,
                )?;
                inserted += 1;
            }
            Some(s)
                if s.root_id != root_id
                    || s.size != entry.size
                    || s.mtime_ns != entry.mtime_ns
                    || s.pipeline_version != PIPELINE_VERSION =>
            {
                files::mark_changed(&tx, s.id, root_id, entry.size, entry.mtime_ns, scan_id)?;
                changed += 1;
            }
            Some(s) => {
                files::mark_seen(&tx, s.id, scan_id)?;
                unchanged += 1;
            }
        }
    }
    tx.execute(
        "UPDATE roots SET last_full_scan_at = unixepoch() WHERE id = ?1",
        [root_id],
    )?;
    let unseen = files::unseen(&tx, root_id, scan_id)?;
    tx.commit()?;
    Ok(Reconciled {
        inserted,
        changed,
        unchanged,
        unseen,
    })
}

/// Totals from [`reconcile_all`].
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ScanSummary {
    pub inserted: u32,
    pub changed: u32,
    pub unchanged: u32,
    pub moved: u32,
    pub removed: u32,
}

/// One scan of every enabled root: probe it, reconcile it, then settle all the
/// deletion candidates together so a move between roots is matched. A root that
/// is missing or unreadable is skipped with its rows intact and its status set.
pub fn reconcile_all(conn: &mut Connection, options: &IndexRootOptions) -> Result<ScanSummary> {
    let scan_id = next_scan_id(conn)?;
    let mut summary = ScanSummary::default();
    let mut unseen = Vec::new();
    for root in roots::list(conn)?.into_iter().filter(|r| r.enabled) {
        let access = FsProbe.probe(&root.path);
        roots::set_access(conn, root.id, &access)?;
        if access != RootAccess::Ok {
            continue;
        }
        let report = reconcile_root(conn, root.id, &root.path, options, scan_id)?;
        summary.inserted += report.inserted;
        summary.changed += report.changed;
        summary.unchanged += report.unchanged;
        unseen.extend(report.unseen);
    }
    let resolved = resolve_moves(conn, unseen, scan_id)?;
    summary.moved = resolved.moved;
    summary.removed = resolved.removed;
    Ok(summary)
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Resolved {
    pub moved: u32,
    pub removed: u32,
}

/// Settles deletion candidates. One whose bytes turned up under a new path (a
/// brand-new `pending` row of the same size, kind and blake3 hash) is renamed
/// in place, keeping its chunks and vectors; the rest are deleted with
/// everything derived from them.
///
/// Only same-size new rows are hashed, so a scan with no deletions reads
/// nothing. A candidate that was never hashed (filename-only kinds) cannot be
/// matched and is deleted; its replacement costs one filename chunk.
pub fn resolve_moves(
    conn: &mut Connection,
    unseen: Vec<files::Missing>,
    scan_id: i64,
) -> Result<Resolved> {
    let mut out = Resolved::default();
    for gone in unseen {
        if let Some(new_id) = find_new_home(conn, &gone)? {
            files::rename_file(conn, gone.id, new_id, scan_id)?;
            out.moved += 1;
        } else {
            files::delete_file(conn, gone.id)?;
            out.removed += 1;
        }
    }
    Ok(out)
}

fn find_new_home(conn: &Connection, gone: &files::Missing) -> Result<Option<i64>> {
    let Some(hash) = gone.content_hash.as_deref() else {
        return Ok(None);
    };
    for (id, path) in files::new_pending_of_size(conn, gone.size)? {
        if discovery::classify(&path, &[]).as_str() == gone.kind
            && hash_file(&path).is_ok_and(|h| h.as_slice() == hash)
        {
            return Ok(Some(id));
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::IndexingConfig;
    use crate::db::{self, roots};
    use crate::embed::{CountingEmbedder, FakeEmbedder};
    use crate::index::pipeline::{IndexContext, IndexSummary, index_root};
    use crate::search::fts::search_fts;
    use std::fs;
    use std::sync::Arc;

    struct Setup {
        _db_dir: tempfile::TempDir,
        root_dir: tempfile::TempDir,
        conn: Connection,
        root: roots::Root,
        embedder: Arc<CountingEmbedder<FakeEmbedder>>,
        options: IndexRootOptions,
    }

    impl Setup {
        fn new() -> Self {
            let db_dir = tempfile::tempdir().unwrap();
            let conn = db::open(&db_dir.path().join("magi.db")).unwrap();
            let root_dir = tempfile::tempdir().unwrap();
            let root = roots::add(&conn, root_dir.path()).unwrap();
            Self {
                _db_dir: db_dir,
                root_dir,
                conn,
                root,
                embedder: Arc::new(FakeEmbedder::counting()),
                options: IndexRootOptions::from_config(&IndexingConfig::default()).unwrap(),
            }
        }

        fn write(&self, name: &str, text: &str) {
            fs::write(self.root_dir.path().join(name), text).unwrap();
        }

        fn run(&mut self) -> IndexSummary {
            let scan_id = next_scan_id(&self.conn).unwrap();
            index_root(
                &mut self.conn,
                self.root.id,
                &self.root.path,
                &self.options,
                scan_id,
                &IndexContext::new(self.embedder.clone()),
            )
            .unwrap()
        }

        fn count(&self, sql: &str) -> i64 {
            self.conn.query_row(sql, [], |r| r.get(0)).unwrap()
        }

        fn status(&self) -> String {
            self.conn
                .query_row("SELECT status FROM roots", [], |r| r.get(0))
                .unwrap()
        }

        fn hits(&self, query: &str) -> usize {
            search_fts(&self.conn, query, 10).unwrap().len()
        }
    }

    /// SPEC.md §7 M5 item 7: a deleted file leaves nothing behind.
    #[test]
    fn a_file_deleted_between_runs_is_removed_everywhere() {
        let mut s = Setup::new();
        s.write("keep.txt", "the keeper");
        s.write("gone.txt", "zebra stripes");
        s.run();
        assert_eq!(s.hits("zebra"), 1);

        fs::remove_file(s.root_dir.path().join("gone.txt")).unwrap();
        let summary = s.run();

        assert_eq!(summary.removed, 1);
        assert_eq!(s.hits("zebra"), 0);
        assert_eq!(s.count("SELECT COUNT(*) FROM files"), 1);
        assert_eq!(
            s.count("SELECT COUNT(*) FROM chunks WHERE file_id NOT IN (SELECT id FROM files)"),
            0
        );
        assert_eq!(
            s.count("SELECT COUNT(*) FROM vec_text WHERE chunk_id NOT IN (SELECT id FROM chunks)"),
            0
        );
    }

    /// Items 5 and 8: a rename keeps the row, chunks and vectors, and embeds
    /// nothing; the new name is searchable and the old one is not.
    #[test]
    fn a_rename_is_followed_in_place_without_re_embedding() {
        let mut s = Setup::new();
        s.write("apple.txt", "orchard notes");
        s.run();
        let id = s.count("SELECT id FROM files");
        let embedded = s.embedder.chunks();

        fs::rename(
            s.root_dir.path().join("apple.txt"),
            s.root_dir.path().join("banana.txt"),
        )
        .unwrap();
        let summary = s.run();

        assert_eq!((summary.moved, summary.removed, summary.indexed), (1, 0, 0));
        assert_eq!(s.embedder.chunks(), embedded, "nothing re-embedded");
        assert_eq!(s.count("SELECT COUNT(*) FROM files"), 1);
        assert_eq!(s.count("SELECT id FROM files"), id, "same row");
        let name: String = s
            .conn
            .query_row("SELECT file_name FROM files", [], |r| r.get(0))
            .unwrap();
        assert_eq!(name, "banana.txt");
        assert_eq!(s.hits("banana"), 1, "found by its new name");
        assert_eq!(s.hits("apple"), 0, "and not by the old one");
        assert_eq!(s.hits("orchard"), 1, "content still searchable");
    }

    /// Item 15: the same bytes turning up under another root are a move too;
    /// the caller scans both roots before settling the candidates.
    #[test]
    fn a_move_between_roots_reassigns_the_row() {
        let mut s = Setup::new();
        let other_dir = tempfile::tempdir().unwrap();
        let other = roots::add(&s.conn, other_dir.path()).unwrap();
        s.write("report.txt", "quarterly figures");
        s.run();
        let id = s.count("SELECT id FROM files");
        let embedded = s.embedder.chunks();

        fs::rename(
            s.root_dir.path().join("report.txt"),
            other_dir.path().join("report.txt"),
        )
        .unwrap();
        let scan_id = next_scan_id(&s.conn).unwrap();
        let mut unseen = Vec::new();
        for r in [&s.root, &other] {
            let report = reconcile_root(&mut s.conn, r.id, &r.path, &s.options, scan_id).unwrap();
            unseen.extend(report.unseen);
        }
        let resolved = resolve_moves(&mut s.conn, unseen, scan_id).unwrap();

        assert_eq!(
            resolved,
            Resolved {
                moved: 1,
                removed: 0
            }
        );
        assert_eq!(s.count("SELECT COUNT(*) FROM files"), 1);
        assert_eq!(s.count("SELECT id FROM files"), id);
        assert_eq!(
            s.count(&format!(
                "SELECT COUNT(*) FROM files WHERE root_id = {}",
                other.id
            )),
            1
        );
        assert_eq!(s.embedder.chunks(), embedded);
    }

    /// A file that was edited while it moved is not the same bytes: it is a
    /// deletion plus a new file.
    #[test]
    fn a_moved_and_edited_file_is_reindexed() {
        let mut s = Setup::new();
        s.write("a.txt", "first version");
        s.run();
        fs::remove_file(s.root_dir.path().join("a.txt")).unwrap();
        s.write("b.txt", "other text");
        let summary = s.run();

        assert_eq!((summary.moved, summary.removed, summary.indexed), (0, 1, 1));
        assert_eq!(s.hits("first"), 0);
        assert_eq!(s.hits("other"), 1);
    }

    /// Item 4: content changed while stopped is re-embedded, and the old text
    /// is no longer searchable.
    #[test]
    fn a_file_edited_between_runs_is_reindexed() {
        let mut s = Setup::new();
        s.write("n.txt", "alpha beta");
        s.run();
        s.write("n.txt", "gamma delta extra");
        let summary = s.run();

        assert_eq!(summary.indexed, 1);
        assert_eq!(s.hits("alpha"), 0);
        assert_eq!(s.hits("gamma"), 1);
    }

    /// Item 13: a file that becomes excluded is deleted by the next scan.
    #[test]
    fn a_newly_excluded_file_is_removed() {
        let mut s = Setup::new();
        s.write("keep.txt", "kept words");
        s.write("draft.txt", "temporary words");
        s.options.exclude_globs.clear();
        s.run();
        assert_eq!(s.hits("temporary"), 1);

        s.options
            .exclude_globs
            .push(glob::Pattern::new("**/draft.txt").unwrap());
        let summary = s.run();

        assert_eq!(summary.removed, 1);
        assert_eq!(s.hits("temporary"), 0);
        assert_eq!(s.hits("kept"), 1);
    }

    /// A row written by an older pipeline is re-queued by the scan even when
    /// size and mtime match, and re-extracted: that is what bumping
    /// `PIPELINE_VERSION` is for. Once current it is left alone again.
    #[test]
    fn a_stale_pipeline_version_is_reindexed_once() {
        let mut s = Setup::new();
        s.write("n.txt", "steady text");
        s.run();
        s.conn
            .execute("UPDATE files SET pipeline_version = 0", [])
            .unwrap();

        assert_eq!(s.run().indexed, 1);
        assert_eq!(
            s.count("SELECT pipeline_version FROM files"),
            PIPELINE_VERSION
        );
        let embedded = s.embedder.chunks();
        assert_eq!(s.run().unchanged, 1);
        assert_eq!(s.embedder.chunks(), embedded);
    }

    /// Item 12 and SPEC.md §5.4 "Missing roots": an unplugged root keeps its
    /// index but its results are hidden; when it returns they come back.
    #[test]
    fn a_missing_root_keeps_its_rows_and_hides_its_results() {
        let mut s = Setup::new();
        s.write("n.txt", "findable words");
        s.run();
        let away = tempfile::tempdir().unwrap();
        let parked = away.path().join("parked");
        fs::rename(s.root_dir.path(), &parked).unwrap();

        let summary = s.run();

        assert_eq!(summary, IndexSummary::default());
        assert_eq!(s.count("SELECT COUNT(*) FROM files"), 1, "index kept");
        assert_eq!(s.status(), "missing");
        assert_eq!(s.hits("findable"), 0);

        fs::rename(&parked, s.root_dir.path()).unwrap();
        s.run();
        assert_eq!(s.status(), "ok");
        assert_eq!(s.hits("findable"), 1);
    }

    #[test]
    fn a_disabled_root_is_hidden_from_search() {
        let mut s = Setup::new();
        s.write("n.txt", "findable words");
        s.run();
        s.conn.execute("UPDATE roots SET enabled = 0", []).unwrap();
        assert_eq!(s.hits("findable"), 0);
    }

    #[test]
    fn scan_ids_increase_and_the_scan_time_is_recorded() {
        let mut s = Setup::new();
        assert_eq!(next_scan_id(&s.conn).unwrap(), 1);
        assert_eq!(next_scan_id(&s.conn).unwrap(), 2);
        s.run();
        let scanned: Option<i64> = s
            .conn
            .query_row("SELECT last_full_scan_at FROM roots", [], |r| r.get(0))
            .unwrap();
        assert!(scanned.is_some());
    }
}
