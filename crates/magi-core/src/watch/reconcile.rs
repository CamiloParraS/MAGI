//! Reconciliation scans: diff a walk of a root against its rows (SPEC.md §5.4
//! startup sequence step 4) and detect moves. Plain functions over a
//! connection; the runtime and the watcher call them.

use std::path::{Path, PathBuf};

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

enum EntryOutcome {
    Inserted,
    Changed,
    Unchanged,
}

/// Brings one file's row in line with what was found on disk.
fn reconcile_entry(
    conn: &Connection,
    root_id: i64,
    root_path: &Path,
    entry: &discovery::WalkEntry,
    scan_id: i64,
) -> Result<EntryOutcome> {
    Ok(match files::get_stored(conn, &entry.path)? {
        None => {
            let rel = entry.path.strip_prefix(root_path).unwrap_or(&entry.path);
            files::insert_pending(
                conn,
                root_id,
                &entry.path,
                rel,
                entry.size,
                entry.mtime_ns,
                scan_id,
            )?;
            EntryOutcome::Inserted
        }
        Some(s)
            if s.root_id != root_id
                || s.size != entry.size
                || s.mtime_ns != entry.mtime_ns
                || s.pipeline_version != PIPELINE_VERSION =>
        {
            files::mark_changed(conn, s.id, root_id, entry.size, entry.mtime_ns, scan_id)?;
            EntryOutcome::Changed
        }
        Some(s) => {
            files::mark_seen(conn, s.id, scan_id)?;
            EntryOutcome::Unchanged
        }
    })
}

/// Reconciles only `paths` (what the watcher reported): each is looked at on
/// disk, a folder is walked, and whatever was at or under a path but is no
/// longer found there (deleted, moved away, now excluded) is settled by
/// [`resolve_moves`], so a rename is matched by content instead of re-embedded.
/// A path outside every usable root is ignored; one that is a root itself
/// triggers a full [`reconcile_all`], which probes it.
pub fn reconcile_paths(
    conn: &mut Connection,
    options: &IndexRootOptions,
    paths: &[PathBuf],
) -> Result<ScanSummary> {
    let scan = scan_paths(conn, options, paths)?;
    let mut summary = scan.summary;
    let resolved = resolve_moves(conn, scan.unseen, scan.scan_id)?;
    summary.moved += resolved.moved;
    summary.removed += resolved.removed;
    Ok(summary)
}

/// What [`scan_paths`] found and left undecided.
pub struct PathScan {
    pub summary: ScanSummary,
    /// Rows at or under the paths that were not found there.
    pub unseen: Vec<files::Missing>,
    pub scan_id: i64,
}

/// The scanning half of [`reconcile_paths`]: queues what is new or changed and
/// returns what disappeared without deciding it, so the caller can hold it for
/// a moment. A move between roots is reported by two watchers that debounce
/// independently, so the removal can arrive before the creation.
pub fn scan_paths(
    conn: &mut Connection,
    options: &IndexRootOptions,
    paths: &[PathBuf],
) -> Result<PathScan> {
    let usable: Vec<_> = roots::list(conn)?
        .into_iter()
        .filter(|r| r.enabled && matches!(r.status.as_str(), "ok" | "watch_failed"))
        .collect();
    if paths.iter().any(|p| usable.iter().any(|r| r.path == *p)) {
        let scan_id = next_scan_id(conn)?;
        return Ok(PathScan {
            summary: reconcile_all(conn, options)?,
            unseen: Vec::new(),
            scan_id,
        });
    }
    let scan_id = next_scan_id(conn)?;
    let walk_options = options.walk_options();
    let mut summary = ScanSummary::default();
    let tx = conn.transaction()?;

    let mut scanned = Vec::new();
    for path in paths {
        let Some(root) = usable.iter().find(|r| path.starts_with(&r.path)) else {
            continue;
        };
        let entries = if !discovery::is_wanted(&root.path, path, &walk_options) {
            Vec::new()
        } else {
            match std::fs::metadata(path) {
                Ok(m) if m.is_dir() => discovery::walk_under(&root.path, path, &walk_options)?,
                Ok(m) if m.is_file() => discovery::stat(path).into_iter().collect(),
                _ => Vec::new(),
            }
        };
        for entry in &entries {
            match reconcile_entry(&tx, root.id, &root.path, entry, scan_id)? {
                EntryOutcome::Inserted => summary.inserted += 1,
                EntryOutcome::Changed => summary.changed += 1,
                EntryOutcome::Unchanged => summary.unchanged += 1,
            }
        }
        scanned.push(path);
    }
    // After every path is marked, so a file that turned up under one path is
    // not mistaken for a deletion under another.
    let mut seen = std::collections::HashSet::new();
    let mut unseen = Vec::new();
    for path in scanned {
        for gone in files::unseen_under(&tx, path, scan_id)? {
            if seen.insert(gone.id) {
                unseen.push(gone);
            }
        }
    }
    tx.commit()?;
    Ok(PathScan {
        summary,
        unseen,
        scan_id,
    })
}

/// A deletion candidate waiting to see whether its bytes turn up elsewhere.
pub struct Held {
    pub gone: files::Missing,
    pub scan_id: i64,
    pub until: std::time::Instant,
}

/// Settles held candidates. One whose bytes have turned up (a new `pending`
/// row of the same size, kind and hash) is renamed in place; one seen again
/// (recreated, as an editor's save does) is dropped from the list; one still
/// unmatched when its time is up is deleted.
pub fn settle_held(
    conn: &mut Connection,
    held: &mut Vec<Held>,
    now: std::time::Instant,
) -> Result<Resolved> {
    let mut out = Resolved::default();
    for item in std::mem::take(held) {
        if !files::is_unseen_since(conn, item.gone.id, item.scan_id)? {
            continue;
        }
        if let Some(new_id) = find_new_home(conn, &item.gone)? {
            files::rename_file(conn, item.gone.id, new_id, item.scan_id)?;
            out.moved += 1;
        } else if item.until <= now {
            files::delete_file(conn, item.gone.id)?;
            out.removed += 1;
        } else {
            held.push(item);
        }
    }
    Ok(out)
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
        match reconcile_entry(&tx, root_id, root_path, entry, scan_id)? {
            EntryOutcome::Inserted => inserted += 1,
            EntryOutcome::Changed => changed += 1,
            EntryOutcome::Unchanged => unchanged += 1,
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
    use std::path::PathBuf;
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

    fn reconcile_these(s: &mut Setup, names: &[&str]) -> ScanSummary {
        let paths: Vec<PathBuf> = names.iter().map(|n| s.root.path.join(n)).collect();
        let summary = reconcile_paths(&mut s.conn, &s.options, &paths).unwrap();
        // What the scheduler and workers would do next.
        s.run();
        summary
    }

    /// The watcher's view of a rename: the old path and the new one, together.
    #[test]
    fn a_reported_rename_is_a_move_not_a_re_embed() {
        let mut s = Setup::new();
        s.write("old_name.txt", "orchard notes");
        s.run();
        let embedded = s.embedder.chunks();
        fs::rename(
            s.root_dir.path().join("old_name.txt"),
            s.root_dir.path().join("new_name.txt"),
        )
        .unwrap();

        let summary = reconcile_these(&mut s, &["old_name.txt", "new_name.txt"]);

        assert_eq!((summary.moved, summary.removed), (1, 0));
        assert_eq!(s.embedder.chunks(), embedded);
        assert_eq!(s.hits("new_name"), 1);
        assert_eq!(s.hits("old_name"), 0);
    }

    #[test]
    fn a_renamed_folder_moves_every_file_in_it() {
        let mut s = Setup::new();
        fs::create_dir(s.root_dir.path().join("before")).unwrap();
        s.write("before/a.txt", "alpha words");
        s.write("before/b.txt", "bravo words");
        s.run();
        let embedded = s.embedder.chunks();
        fs::rename(
            s.root_dir.path().join("before"),
            s.root_dir.path().join("after"),
        )
        .unwrap();

        let summary = reconcile_these(&mut s, &["before", "after"]);

        assert_eq!((summary.moved, summary.removed), (2, 0));
        assert_eq!(s.embedder.chunks(), embedded, "nothing re-embedded");
        assert_eq!(
            s.count("SELECT COUNT(*) FROM files WHERE path LIKE '%after%'"),
            2
        );
        assert_eq!(s.hits("alpha"), 1);
    }

    #[test]
    fn a_reported_delete_removes_the_file_and_a_folder_delete_removes_its_contents() {
        let mut s = Setup::new();
        fs::create_dir(s.root_dir.path().join("dir")).unwrap();
        s.write("dir/a.txt", "alpha words");
        s.write("dir/b.txt", "bravo words");
        s.write("solo.txt", "charlie words");
        s.run();
        fs::remove_dir_all(s.root_dir.path().join("dir")).unwrap();
        fs::remove_file(s.root_dir.path().join("solo.txt")).unwrap();

        let summary = reconcile_these(&mut s, &["dir", "solo.txt"]);

        assert_eq!(summary.removed, 3);
        assert_eq!(s.count("SELECT COUNT(*) FROM files"), 0);
    }

    #[test]
    fn a_reported_create_and_modify_queue_only_that_file() {
        let mut s = Setup::new();
        s.write("keep.txt", "unchanged words");
        s.write("edit.txt", "first version");
        s.run();
        s.write("edit.txt", "second version, longer");
        s.write("fresh.txt", "brand new words");

        let summary = reconcile_paths(
            &mut s.conn,
            &s.options,
            &[s.root.path.join("edit.txt"), s.root.path.join("fresh.txt")],
        )
        .unwrap();

        assert_eq!((summary.inserted, summary.changed), (1, 1));
        assert_eq!(
            s.count("SELECT COUNT(*) FROM files WHERE state = 'pending'"),
            2
        );
    }

    /// Moving a file into an excluded folder removes it; a path outside every
    /// root is not looked at.
    #[test]
    fn a_path_that_is_excluded_or_outside_every_root_is_handled() {
        let mut s = Setup::new();
        s.options
            .exclude_globs
            .push(glob::Pattern::new("**/skipme/**").unwrap());
        fs::create_dir(s.root_dir.path().join("skipme")).unwrap();
        s.write("a.txt", "alpha words");
        s.run();
        fs::rename(
            s.root_dir.path().join("a.txt"),
            s.root_dir.path().join("skipme/a.txt"),
        )
        .unwrap();
        let elsewhere = tempfile::tempdir().unwrap();
        fs::write(elsewhere.path().join("stray.txt"), "x").unwrap();

        let summary = reconcile_paths(
            &mut s.conn,
            &s.options,
            &[
                s.root.path.join("a.txt"),
                s.root.path.join("skipme/a.txt"),
                elsewhere.path().join("stray.txt"),
            ],
        )
        .unwrap();

        assert_eq!((summary.removed, summary.inserted), (1, 0));
        assert_eq!(s.count("SELECT COUNT(*) FROM files"), 0);
    }

    fn held_from(scan: PathScan, until: std::time::Instant) -> Vec<Held> {
        scan.unseen
            .into_iter()
            .map(|gone| Held {
                gone,
                scan_id: scan.scan_id,
                until,
            })
            .collect()
    }

    /// A removal reported in one batch is claimed by the creation reported in
    /// a later one (two roots' watchers debounce separately), and is deleted
    /// only if nothing claims it before the hold is over.
    #[test]
    fn a_held_removal_is_claimed_by_a_later_creation_or_expires() {
        use std::time::{Duration, Instant};
        let mut s = Setup::new();
        s.write("a.txt", "alpha words");
        s.write("b.txt", "bravo words");
        s.run();
        let embedded = s.embedder.chunks();
        let now = Instant::now();
        let hold = now + Duration::from_secs(5);

        // Batch 1: a.txt is gone. Nothing to match it with yet.
        fs::rename(
            s.root_dir.path().join("a.txt"),
            s.root_dir.path().join("moved.txt"),
        )
        .unwrap();
        let scan = scan_paths(&mut s.conn, &s.options, &[s.root.path.join("a.txt")]).unwrap();
        let mut held = held_from(scan, hold);
        let r = settle_held(&mut s.conn, &mut held, now).unwrap();
        assert_eq!((r.moved, r.removed, held.len()), (0, 0, 1));

        // Batch 2: the creation arrives and claims it.
        scan_paths(&mut s.conn, &s.options, &[s.root.path.join("moved.txt")]).unwrap();
        let r = settle_held(&mut s.conn, &mut held, now).unwrap();
        assert_eq!((r.moved, r.removed, held.len()), (1, 0, 0));
        assert_eq!(s.embedder.chunks(), embedded, "nothing re-embedded");
        assert_eq!(
            s.count("SELECT COUNT(*) FROM files WHERE file_name = 'moved.txt'"),
            1
        );

        // b.txt is removed and nothing ever claims it: deleted once time is up.
        fs::remove_file(s.root_dir.path().join("b.txt")).unwrap();
        let scan = scan_paths(&mut s.conn, &s.options, &[s.root.path.join("b.txt")]).unwrap();
        let mut held = held_from(scan, hold);
        assert_eq!(settle_held(&mut s.conn, &mut held, now).unwrap().removed, 0);
        let later = now + Duration::from_secs(6);
        assert_eq!(
            settle_held(&mut s.conn, &mut held, later).unwrap().removed,
            1
        );
        assert_eq!(s.count("SELECT COUNT(*) FROM files"), 1);
    }

    /// An editor's atomic save removes and recreates the path: the recreated
    /// file is the same row, and the hold must not delete it.
    #[test]
    fn a_held_file_that_reappears_at_its_path_is_not_deleted() {
        use std::time::{Duration, Instant};
        let mut s = Setup::new();
        s.write("doc.txt", "original words");
        s.run();
        let now = Instant::now();
        fs::remove_file(s.root_dir.path().join("doc.txt")).unwrap();
        let scan = scan_paths(&mut s.conn, &s.options, &[s.root.path.join("doc.txt")]).unwrap();
        let mut held = held_from(scan, now + Duration::from_secs(5));

        s.write("doc.txt", "rewritten words, longer");
        scan_paths(&mut s.conn, &s.options, &[s.root.path.join("doc.txt")]).unwrap();
        let r = settle_held(&mut s.conn, &mut held, now + Duration::from_secs(6)).unwrap();

        assert_eq!((r.moved, r.removed, held.len()), (0, 0, 0));
        assert_eq!(s.count("SELECT COUNT(*) FROM files"), 1);
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
