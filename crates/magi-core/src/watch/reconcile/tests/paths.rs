//! Reported paths: what the watcher hands over, resolved without a full walk.

use super::*;

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

/// The actual shape of the CI failure: `notify`'s debouncer commonly
/// reports both halves of a same-root rename in one batch (one
/// `scan_paths` call, `held` empty). The match must be found and applied
/// within that single call's own transaction — not left to a later,
/// separate step — so a plain `pending` row for the new name is never
/// committed for a scheduler thread to pick up and re-embed.
#[test]
fn scan_paths_resolves_both_halves_of_one_batch_before_committing() {
    let mut s = Setup::new();
    s.write("before.txt", "portable words");
    s.run();
    let id = s.count("SELECT id FROM files");
    let embedded = s.embedder.chunks();

    fs::rename(
        s.root_dir.path().join("before.txt"),
        s.root_dir.path().join("after.txt"),
    )
    .unwrap();
    let scan = scan_paths(
        &mut s.conn,
        &s.options,
        &[
            s.root.path.join("before.txt"),
            s.root.path.join("after.txt"),
        ],
    )
    .unwrap();

    assert_eq!(scan.summary.moved, 1);
    assert!(scan.unseen.is_empty(), "resolved, not left for later");
    assert_eq!(
        s.count("SELECT COUNT(*) FROM files WHERE state = 'pending'"),
        0,
        "never committed as an unclaimed pending row"
    );
    assert_eq!(s.count("SELECT COUNT(*) FROM files"), 1);
    assert_eq!(s.count("SELECT id FROM files"), id, "same row");
    assert_eq!(s.embedder.chunks(), embedded, "nothing re-embedded");
    assert_eq!(s.hits("after"), 1);
    assert_eq!(s.hits("before"), 0);
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
        .map(|u| u.hold_until(until))
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
    assert_eq!((r, held.len()), (0, 1));

    // Batch 2: the creation arrives and claims it; the hold just lets go.
    let scan = scan_paths(&mut s.conn, &s.options, &[s.root.path.join("moved.txt")]).unwrap();
    assert_eq!(scan.summary.moved, 1);
    let r = settle_held(&mut s.conn, &mut held, now).unwrap();
    assert_eq!((r, held.len()), (0, 0));
    assert_eq!(s.embedder.chunks(), embedded, "nothing re-embedded");
    assert_eq!(
        s.count("SELECT COUNT(*) FROM files WHERE file_name = 'moved.txt'"),
        1
    );

    // b.txt is removed and nothing ever claims it: deleted once time is up.
    fs::remove_file(s.root_dir.path().join("b.txt")).unwrap();
    let scan = scan_paths(&mut s.conn, &s.options, &[s.root.path.join("b.txt")]).unwrap();
    let mut held = held_from(scan, hold);
    assert_eq!(settle_held(&mut s.conn, &mut held, now).unwrap(), 0);
    let later = now + Duration::from_secs(6);
    assert_eq!(settle_held(&mut s.conn, &mut held, later).unwrap(), 1);
    assert_eq!(s.count("SELECT COUNT(*) FROM files"), 1);
}

/// What macOS CI actually saw: `notify-debouncer-full` folds a rename of a
/// file whose queue starts with a `Create` (FSEvents coalesces flags on a
/// fresh file) into a plain `Create` of the new path. The old path is never
/// reported, so the new path must find its vanished twin by itself.
#[test]
fn a_rename_reported_only_by_its_new_path_is_a_move() {
    let mut s = Setup::new();
    s.write("before.txt", "portable words");
    s.run();
    let id = s.count("SELECT id FROM files");
    let embedded = s.embedder.chunks();

    fs::rename(
        s.root_dir.path().join("before.txt"),
        s.root_dir.path().join("after.txt"),
    )
    .unwrap();
    let scan = scan_paths(&mut s.conn, &s.options, &[s.root.path.join("after.txt")]).unwrap();

    assert_eq!(scan.summary.moved, 1);
    assert_eq!(
        s.count("SELECT COUNT(*) FROM files WHERE state = 'pending'"),
        0,
        "never exposed as an unclaimed pending row"
    );
    assert_eq!(s.count("SELECT COUNT(*) FROM files"), 1);
    assert_eq!(s.count("SELECT id FROM files"), id, "same row");
    assert_eq!(s.embedder.chunks(), embedded, "nothing re-embedded");
    assert_eq!(s.hits("after"), 1);
    assert_eq!(s.hits("before"), 0);
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

    assert_eq!((r, held.len()), (0, 0));
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
