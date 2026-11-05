use super::*;
use crate::db::{self, roots};
use std::fs;

struct Fixture {
    _db: tempfile::TempDir,
    dir: tempfile::TempDir,
    conn: Connection,
    root_id: i64,
}

impl Fixture {
    fn new() -> Self {
        let db_dir = tempfile::tempdir().unwrap();
        let conn = db::open(&db_dir.path().join("magi.db")).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let root = roots::add(&conn, dir.path()).unwrap();
        Self {
            _db: db_dir,
            dir,
            conn,
            root_id: root.id,
        }
    }

    /// Writes a file and queues it, as a reconciliation would.
    fn queue(&self, name: &str, text: &str) -> i64 {
        let path = self.dir.path().join(name);
        fs::write(&path, text).unwrap();
        let entry = discovery::stat(&path).unwrap();
        files::insert_pending(
            &self.conn,
            self.root_id,
            &path,
            std::path::Path::new(name),
            entry.size,
            entry.mtime_ns,
            1,
        )
        .unwrap();
        self.conn
            .query_row(
                "SELECT id FROM files WHERE path = ?1",
                [path.to_string_lossy()],
                |r| r.get(0),
            )
            .unwrap()
    }
}

fn extracted_ids(actions: &[Action]) -> Vec<i64> {
    actions
        .iter()
        .filter_map(|a| match a {
            Action::Extract { file, .. } => Some(file.id),
            _ => None,
        })
        .collect()
}

/// A file written a moment ago is not handed out until it has been quiet
/// for the settle window and passed the second size check.
#[test]
fn a_fresh_file_waits_until_it_is_stable() {
    let f = Fixture::new();
    let id = f.queue("new.txt", "hello");
    let mut s = Scheduler::new(8);
    let t0 = Now::current();

    assert!(s.poll(&f.conn, t0).unwrap().is_empty(), "just written");
    assert!(
        s.poll(&f.conn, t0.plus(Duration::from_secs(1)))
            .unwrap()
            .is_empty(),
        "still inside the settle window"
    );
    // Past the deferral: watched, then confirmed a second later.
    assert!(
        s.poll(&f.conn, t0.plus(Duration::from_secs(6)))
            .unwrap()
            .is_empty()
    );
    let ready = s.poll(&f.conn, t0.plus(Duration::from_secs(8))).unwrap();
    assert_eq!(extracted_ids(&ready), vec![id]);
}

/// Item 10's mechanism: a file that grew between the two looks is deferred
/// again, not handed out half-written.
#[test]
fn a_file_that_grows_between_looks_is_deferred_again() {
    let f = Fixture::new();
    let id = f.queue("log.txt", "one");
    let mut s = Scheduler::new(8);
    let t0 = Now::current().plus(Duration::from_secs(10)); // long after the write
    assert!(
        s.poll(&f.conn, t0).unwrap().is_empty(),
        "first look: watched"
    );

    fs::write(f.dir.path().join("log.txt"), "one two three").unwrap();
    let second = t0.plus(Duration::from_secs(2));
    assert!(
        s.poll(&f.conn, second).unwrap().is_empty(),
        "grew: deferred"
    );

    let later = second.plus(Duration::from_secs(20));
    assert!(s.poll(&f.conn, later).unwrap().is_empty(), "watched again");
    let ready = s.poll(&f.conn, later.plus(Duration::from_secs(2))).unwrap();
    assert_eq!(extracted_ids(&ready), vec![id]);
}

/// Files already stat'd and waiting must not start once their root is off.
#[test]
fn waiting_files_of_a_switched_off_root_are_not_handed_out() {
    let f = Fixture::new();
    let id = f.queue("held.txt", "x");
    let mut s = Scheduler::new(8);
    let t0 = Now::current().plus(Duration::from_secs(10));
    assert!(s.poll(&f.conn, t0).unwrap().is_empty(), "watched");

    roots::set_enabled(&f.conn, f.root_id, false).unwrap();
    let later = t0.plus(Duration::from_secs(2));
    assert!(s.poll(&f.conn, later).unwrap().is_empty(), "root is off");

    roots::set_enabled(&f.conn, f.root_id, true).unwrap();
    let again = later.plus(Duration::from_secs(1));
    assert!(s.poll(&f.conn, again).unwrap().is_empty(), "watched anew");
    let ready = s.poll(&f.conn, again.plus(Duration::from_secs(2))).unwrap();
    assert_eq!(extracted_ids(&ready), vec![id]);
}

#[test]
fn a_vanished_file_is_deleted_not_extracted() {
    let f = Fixture::new();
    let id = f.queue("gone.txt", "x");
    fs::remove_file(f.dir.path().join("gone.txt")).unwrap();
    let mut s = Scheduler::new(8);
    assert_eq!(
        s.poll(&f.conn, Now::current()).unwrap(),
        vec![Action::Delete(id)]
    );
    // Held until the writer reports back, so it is not handed out twice.
    assert!(s.poll(&f.conn, Now::current()).unwrap().is_empty());
}

/// Newest first, and never more than the pipeline has room for.
#[test]
fn hands_out_newest_first_within_the_room_available() {
    let f = Fixture::new();
    let old = f.queue("old.txt", "a");
    let new = f.queue("new.txt", "b");
    // Make `old` older on disk and in its row.
    let old_time = std::time::SystemTime::now() - Duration::from_secs(3600);
    fs::File::options()
        .write(true)
        .open(f.dir.path().join("old.txt"))
        .unwrap()
        .set_modified(old_time)
        .unwrap();
    let entry = discovery::stat(&f.dir.path().join("old.txt")).unwrap();
    f.conn
        .execute(
            "UPDATE files SET mtime_ns = ?1 WHERE id = ?2",
            rusqlite::params![entry.mtime_ns, old],
        )
        .unwrap();

    let mut s = Scheduler::new(1);
    let t0 = Now::current().plus(Duration::from_secs(10));
    s.poll(&f.conn, t0).unwrap();
    let ready = s.poll(&f.conn, t0.plus(Duration::from_secs(2))).unwrap();
    assert_eq!(extracted_ids(&ready), vec![new], "one slot: the newer file");

    s.finished(new);
    let ready = s.poll(&f.conn, t0.plus(Duration::from_secs(3))).unwrap();
    assert_eq!(extracted_ids(&ready), vec![old]);
}

/// A file that proved stable while the pipeline was full waits for room
/// without being stat'd again on every poll: once room frees it is handed
/// out as it was (here, even though it was deleted meanwhile — the extract
/// stage and the watcher deal with that).
#[test]
fn a_stable_file_waiting_for_room_is_not_looked_at_again() {
    let f = Fixture::new();
    let first = f.queue("first.txt", "a");
    let second = f.queue("second.txt", "b");
    let mut s = Scheduler::new(1);
    let t0 = Now::current().plus(Duration::from_secs(10));
    s.poll(&f.conn, t0).unwrap();
    let ready = s.poll(&f.conn, t0.plus(Duration::from_secs(2))).unwrap();
    assert_eq!(ready.len(), 1, "one slot");
    let (handed, waiting) = if extracted_ids(&ready) == vec![first] {
        (first, ("second.txt", second))
    } else {
        (second, ("first.txt", first))
    };

    fs::remove_file(f.dir.path().join(waiting.0)).unwrap();
    assert!(
        s.poll(&f.conn, t0.plus(Duration::from_secs(3)))
            .unwrap()
            .is_empty(),
        "still no room: nothing handed out, nothing stat'd"
    );
    s.finished(handed);
    let ready = s.poll(&f.conn, t0.plus(Duration::from_secs(4))).unwrap();
    assert_eq!(extracted_ids(&ready), vec![waiting.1]);
}

/// A modification time in the future must not hold a file back for as long.
#[test]
fn a_file_dated_in_the_future_is_not_held_until_then() {
    let f = Fixture::new();
    let id = f.queue("skewed.txt", "x");
    fs::File::options()
        .write(true)
        .open(f.dir.path().join("skewed.txt"))
        .unwrap()
        .set_modified(std::time::SystemTime::now() + Duration::from_secs(3600))
        .unwrap();
    let mut s = Scheduler::new(8);
    let t0 = Now::current();
    assert!(
        s.poll(&f.conn, t0).unwrap().is_empty(),
        "first look: watched"
    );
    let ready = s.poll(&f.conn, t0.plus(Duration::from_secs(2))).unwrap();
    assert_eq!(extracted_ids(&ready), vec![id]);
}

/// A row waiting out a retry delay is not touched.
#[test]
fn a_row_in_backoff_is_left_alone() {
    let f = Fixture::new();
    let id = f.queue("later.txt", "x");
    files::record_failure(
        &f.conn,
        id,
        FileErrorCode::ReadFailed,
        "locked",
        Now::current().unix_secs,
    )
    .unwrap();
    let mut s = Scheduler::new(8);
    let t0 = Now::current().plus(Duration::from_secs(10));
    s.poll(&f.conn, t0).unwrap();
    assert!(
        s.poll(&f.conn, t0.plus(Duration::from_secs(2)))
            .unwrap()
            .is_empty()
    );
}
