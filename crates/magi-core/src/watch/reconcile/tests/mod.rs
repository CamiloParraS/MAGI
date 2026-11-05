use super::*;
use crate::config::IndexingConfig;
use crate::db::{self, roots};
use crate::embed::{CountingEmbedder, FakeEmbedder};
use crate::index::pipeline::{IndexContext, IndexSummary, index_root};
use crate::search::fts::search_fts;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

mod paths;

/// [`scan_paths`] with the removals settled at once, as the writer does
/// once their hold is up.
fn reconcile_paths(
    conn: &mut Connection,
    options: &IndexRootOptions,
    paths: &[PathBuf],
) -> Result<ScanSummary> {
    let scan = scan_paths(conn, options, paths)?;
    let mut summary = scan.summary;
    summary.removed += remove_unseen(conn, scan.unseen)?;
    Ok(summary)
}

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
            embedder: Arc::new(CountingEmbedder::new(FakeEmbedder)),
            options: IndexRootOptions::from_config(&IndexingConfig::default()).unwrap(),
        }
    }

    fn write(&self, name: &str, text: &str) {
        fs::write(self.root_dir.path().join(name), text).unwrap();
    }

    fn run(&mut self) -> IndexSummary {
        index_root(
            &mut self.conn,
            self.root.id,
            &self.root.path,
            &self.options,
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

#[test]
fn a_scan_counts_the_files_it_finds() {
    let mut s = Setup::new();
    s.write("a.txt", "a");
    s.write("b.txt", "b");
    reconcile_roots(&mut s.conn, &s.options, &[s.root.id]).unwrap();
    assert_eq!(s.options.scan.seen(), 2);
}

/// Quitting mid-scan must not read as "everything I did not reach is gone".
#[test]
fn a_stopped_scan_changes_and_deletes_nothing() {
    let mut s = Setup::new();
    s.write("old.txt", "old");
    s.run();
    fs::remove_file(s.root_dir.path().join("old.txt")).unwrap();
    s.write("new.txt", "new");

    s.options.scan.stop();
    let summary = reconcile_roots(&mut s.conn, &s.options, &[s.root.id]).unwrap();

    assert_eq!(summary, ScanSummary::default());
    assert_eq!(
        s.count("SELECT COUNT(*) FROM files WHERE file_name = 'old.txt'"),
        1
    );
    assert_eq!(
        s.count("SELECT COUNT(*) FROM files WHERE file_name = 'new.txt'"),
        0
    );
}

/// Switching a folder off cancels its scan, not the scan of the others.
#[test]
fn a_switched_off_root_is_skipped_and_the_others_are_scanned() {
    let mut s = Setup::new();
    let other_dir = tempfile::tempdir().unwrap();
    let other = roots::add(&s.conn, other_dir.path()).unwrap();
    s.write("mine.txt", "mine");
    fs::write(other_dir.path().join("theirs.txt"), "theirs").unwrap();

    s.options.scan.switch_off(s.root.id);
    let summary = reconcile_roots(&mut s.conn, &s.options, &[s.root.id, other.id]).unwrap();
    assert_eq!(summary.inserted, 1);
    assert_eq!(
        s.count("SELECT COUNT(*) FROM files WHERE file_name = 'mine.txt'"),
        0
    );

    s.options.scan.switch_on(s.root.id);
    let summary = reconcile_roots(&mut s.conn, &s.options, &[s.root.id]).unwrap();
    assert_eq!(summary.inserted, 1);
}

/// Adding or re-enabling a root walks that root only; a change in another
/// root waits for its watcher or the next full scan.
#[test]
fn reconciling_one_root_leaves_the_others_unwalked() {
    let mut s = Setup::new();
    let other_dir = tempfile::tempdir().unwrap();
    let other = roots::add(&s.conn, other_dir.path()).unwrap();
    s.write("mine.txt", "mine");
    fs::write(other_dir.path().join("theirs.txt"), "theirs").unwrap();

    let mine = reconcile_roots(&mut s.conn, &s.options, &[s.root.id]).unwrap();
    assert_eq!(mine.inserted, 1);
    let theirs = "SELECT COUNT(*) FROM files WHERE file_name = 'theirs.txt'";
    assert_eq!(s.count(theirs), 0);

    let others = reconcile_roots(&mut s.conn, &s.options, &[other.id]).unwrap();
    assert_eq!((others.inserted, others.removed), (1, 0));
    assert_eq!(s.count(theirs), 1);
}

/// Adding a parent of an indexed root reuses the child's rows: its first
/// scan finds them unchanged and embeds only what is new.
#[test]
fn a_collapsed_child_is_not_re_indexed_by_the_parent_scan() {
    let db_dir = tempfile::tempdir().unwrap();
    let mut conn = db::open(&db_dir.path().join("magi.db")).unwrap();
    let options = IndexRootOptions::from_config(&IndexingConfig::default()).unwrap();
    let parent = tempfile::tempdir().unwrap();
    let child_dir = parent.path().join("child");
    fs::create_dir(&child_dir).unwrap();
    fs::write(child_dir.join("old.txt"), "already indexed").unwrap();
    let child = roots::add(&conn, &child_dir).unwrap();
    let ctx = IndexContext::new(Arc::new(FakeEmbedder));
    index_root(&mut conn, child.id, &child.path, &options, &ctx).unwrap();
    fs::write(parent.path().join("new.txt"), "brand new").unwrap();

    let (root, collapsed) = roots::add_collapsing(&conn, parent.path()).unwrap();
    let summary = reconcile_roots(&mut conn, &options, &[root.id]).unwrap();

    assert_eq!(collapsed, vec![child.id]);
    assert_eq!(
        (summary.inserted, summary.changed, summary.unchanged),
        (1, 0, 1),
        "only new.txt is queued; old.txt keeps its index"
    );
}

/// SPEC.md §6.1 recovery: a root that was unreadable or missing and is
/// back is reconciled at the next re-probe; a still-broken one is not.
#[test]
fn a_root_that_comes_back_is_reconciled_by_recover() {
    let mut s = Setup::new();
    s.write("a.txt", "alpha");
    assert!(recover(&mut s.conn, &s.options).unwrap().is_none());

    roots::set_status(&s.conn, s.root.id, roots::Health::PermissionDenied).unwrap();
    let summary = recover(&mut s.conn, &s.options).unwrap().unwrap();
    assert_eq!(summary.inserted, 1);
    assert_eq!(s.status(), "ok");
    assert!(recover(&mut s.conn, &s.options).unwrap().is_none());

    let other = tempfile::tempdir().unwrap();
    let unplugged = roots::add(&s.conn, other.path()).unwrap();
    drop(other);
    roots::set_status(&s.conn, unplugged.id, roots::Health::Missing).unwrap();
    assert!(recover(&mut s.conn, &s.options).unwrap().is_none());
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
    let (mut unseen, mut moved) = (Vec::new(), 0);
    for r in [&s.root, &other] {
        let (found, gone) =
            reconcile_root(&mut s.conn, r.id, &r.path, &s.options, scan_id).unwrap();
        unseen.extend(gone);
        moved += found.moved;
    }
    let removed = remove_unseen(&mut s.conn, unseen).unwrap();

    assert_eq!((moved, removed), (1, 0));
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
        crate::index::PIPELINE_VERSION
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
