//! Decides which `pending` file to work on next and when (SPEC.md §5.4,
//! "Processing a pending file"). Pure logic over a connection and the file
//! system, with the clock passed in, so it is testable without threads; the
//! engine's scheduler thread just calls [`Scheduler::poll`] in a loop.
//!
//! The DB's `pending` rows are the queue and this is a cache in front of them,
//! so nothing is lost if the process dies: a restart rebuilds it.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use rusqlite::Connection;

use crate::db::files::{self, PendingFile};
use crate::discovery::{self, WalkEntry};
use crate::error::Result;

/// A file modified more recently than this is probably still being written.
const SETTLE: Duration = Duration::from_secs(3);
/// Gap between the two size checks that catch a writer that does not update
/// the mtime as it goes.
const RECHECK: Duration = Duration::from_secs(1);
/// How long a file that is still changing waits before it is looked at again.
const DEFER: Duration = Duration::from_secs(5);
/// Files being watched for stability at once, so a huge queue is not stat'd
/// all in one go.
const MAX_WAITING: usize = 1024;
/// Rows fetched beyond the ones already known, per poll.
const FETCH_SLACK: usize = 256;

/// A moment, as both clocks the scheduler needs: monotonic for delays, wall
/// for comparing with file mtimes.
#[derive(Clone, Copy)]
pub struct Now {
    pub at: Instant,
    pub unix_secs: i64,
    pub unix_ns: i64,
}

impl Now {
    pub fn current() -> Self {
        let since_epoch = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default();
        Self {
            at: Instant::now(),
            unix_secs: since_epoch.as_secs() as i64,
            unix_ns: since_epoch.as_nanos() as i64,
        }
    }

    /// This moment, `by` later (for tests that do not want to sleep).
    pub fn plus(self, by: Duration) -> Self {
        Self {
            at: self.at + by,
            unix_secs: self.unix_secs + by.as_secs() as i64,
            unix_ns: self.unix_ns + by.as_nanos() as i64,
        }
    }
}

/// What to do about a file. Every action is followed by the writer's
/// [`Scheduler::finished`] for the same id.
#[derive(Debug, PartialEq)]
pub enum Action {
    /// Stable: read, extract, embed and store it.
    Extract { file: PendingFile, entry: WalkEntry },
    /// Gone from disk: delete its row.
    Delete(i64),
    /// Could not be stat'd for a reason that may pass: back off and retry.
    Fail { id: i64, message: String },
}

enum Hold {
    /// Handed to the pipeline; released by [`Scheduler::finished`].
    InFlight,
    /// Stat'd once; looked at again at `check_at` to see whether it grew.
    Watching {
        file: PendingFile,
        size: u64,
        check_at: Instant,
    },
    /// Still changing; not looked at before `until`.
    Deferred { file: PendingFile, until: Instant },
}

pub struct Scheduler {
    held: HashMap<i64, Hold>,
    max_in_flight: usize,
}

impl Scheduler {
    pub fn new(max_in_flight: usize) -> Self {
        Self {
            held: HashMap::new(),
            max_in_flight,
        }
    }

    /// The writer is done with `id` (stored, deleted, or put back for a retry).
    pub fn finished(&mut self, id: i64) {
        self.held.remove(&id);
    }

    pub fn in_flight(&self) -> usize {
        self.held
            .values()
            .filter(|h| matches!(h, Hold::InFlight))
            .count()
    }

    /// One pass: notice new `pending` rows, re-check waiting ones, and return
    /// what is ready, newest first, up to the room left in the pipeline.
    pub fn poll(&mut self, conn: &Connection, now: Now) -> Result<Vec<Action>> {
        let mut actions = Vec::new();
        let mut room = self.max_in_flight.saturating_sub(self.in_flight());

        // Re-check what was waiting, most recently modified first.
        let mut due: Vec<(i64, PendingFile)> = self
            .held
            .iter()
            .filter_map(|(&id, hold)| match hold {
                Hold::Watching { file, check_at, .. } if *check_at <= now.at => {
                    Some((id, file.clone()))
                }
                Hold::Deferred { file, until } if *until <= now.at => Some((id, file.clone())),
                _ => None,
            })
            .collect();
        due.sort_by_key(|(_, f)| std::cmp::Reverse(f.mtime_ns));
        for (id, file) in due {
            let previous = match self.held.get(&id) {
                Some(Hold::Watching { size, .. }) => Some(*size),
                _ => None,
            };
            match discovery::stat(&file.path) {
                Err(e) => {
                    actions.push(gone_or_failed(id, &e));
                    self.held.insert(id, Hold::InFlight);
                }
                Ok(entry) => {
                    if let Some(action) = self.judge(&file, entry, previous, now, &mut room) {
                        actions.push(action);
                    }
                }
            }
        }

        // New rows. The limit leaves room for the known ones at the front.
        let waiting = self.held.len() - self.in_flight();
        if waiting < MAX_WAITING {
            let limit = (self.held.len() + FETCH_SLACK.min(MAX_WAITING - waiting)) as u32;
            for file in files::next_pending(conn, now.unix_secs, limit)? {
                if self.held.contains_key(&file.id) {
                    continue;
                }
                if self.held.len() - self.in_flight() >= MAX_WAITING {
                    break;
                }
                let id = file.id;
                match discovery::stat(&file.path) {
                    Err(e) => {
                        actions.push(gone_or_failed(id, &e));
                        self.held.insert(id, Hold::InFlight);
                    }
                    Ok(entry) => {
                        if let Some(action) = self.judge(&file, entry, None, now, &mut room) {
                            actions.push(action);
                        }
                    }
                }
            }
        }
        Ok(actions)
    }

    /// Settles one stat'd file: defer it, watch it for a second look, or hand
    /// it out if it has proven stable and there is room.
    fn judge(
        &mut self,
        file: &PendingFile,
        entry: WalkEntry,
        previous_size: Option<u64>,
        now: Now,
        room: &mut usize,
    ) -> Option<Action> {
        let id = file.id;
        let age = Duration::from_nanos(now.unix_ns.saturating_sub(entry.mtime_ns).max(0) as u64);
        // Younger than the settle window, or grown since the first look.
        if age < SETTLE || previous_size.is_some_and(|s| s != entry.size) {
            self.held.insert(
                id,
                Hold::Deferred {
                    file: file.clone(),
                    until: now.at + DEFER,
                },
            );
            return None;
        }
        if previous_size.is_none() {
            self.held.insert(
                id,
                Hold::Watching {
                    file: file.clone(),
                    size: entry.size,
                    check_at: now.at + RECHECK,
                },
            );
            return None;
        }
        // Stable. No room yet: leave it watched and look again next poll.
        if *room == 0 {
            self.held.insert(
                id,
                Hold::Watching {
                    file: file.clone(),
                    size: entry.size,
                    check_at: now.at,
                },
            );
            return None;
        }
        *room -= 1;
        self.held.insert(id, Hold::InFlight);
        Some(Action::Extract {
            file: file.clone(),
            entry,
        })
    }
}

fn gone_or_failed(id: i64, e: &std::io::Error) -> Action {
    if e.kind() == std::io::ErrorKind::NotFound {
        Action::Delete(id)
    } else {
        Action::Fail {
            id,
            message: e.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
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

    /// A row waiting out a retry delay is not touched.
    #[test]
    fn a_row_in_backoff_is_left_alone() {
        let f = Fixture::new();
        let id = f.queue("later.txt", "x");
        files::record_failure(&f.conn, id, "locked", Now::current().unix_secs).unwrap();
        let mut s = Scheduler::new(8);
        let t0 = Now::current().plus(Duration::from_secs(10));
        s.poll(&f.conn, t0).unwrap();
        assert!(
            s.poll(&f.conn, t0.plus(Duration::from_secs(2)))
                .unwrap()
                .is_empty()
        );
    }
}
