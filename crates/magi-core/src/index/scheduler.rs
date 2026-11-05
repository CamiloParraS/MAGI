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
use crate::db::roots;
use crate::discovery::{self, WalkEntry};
use crate::dto::FileErrorCode;
use crate::error::Result;

/// A file modified more recently than this is probably still being written.
const SETTLE: Duration = Duration::from_secs(3);
/// Gap between the two size checks that catch a writer that does not update
/// the mtime as it goes.
const RECHECK: Duration = Duration::from_secs(1);
/// How long a file that is still changing waits before it is looked at again.
const DEFER: Duration = Duration::from_secs(5);
/// Modification times this far ahead of the clock count as "just now".
const CLOCK_SLOP_NS: i64 = 2_000_000_000;
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
    Fail {
        id: i64,
        code: FileErrorCode,
        message: String,
    },
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
    /// Proven stable while the pipeline was full: handed out as soon as there
    /// is room, without another stat.
    Ready { file: PendingFile, entry: WalkEntry },
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

    fn in_flight(&self) -> usize {
        self.held
            .values()
            .filter(|h| matches!(h, Hold::InFlight))
            .count()
    }

    /// One pass: notice new `pending` rows, re-check waiting ones, and return
    /// what is ready, newest first, up to the room left in the pipeline.
    pub fn poll(&mut self, conn: &Connection, now: Now) -> Result<Vec<Action>> {
        // A root switched off (or gone unreadable) since a file was held: let
        // go of its waiting files. They are looked at again if the root is back.
        let usable: Vec<i64> = roots::list(conn)?
            .into_iter()
            .filter(|r| r.indexable())
            .map(|r| r.id)
            .collect();
        self.held.retain(|_, hold| match hold {
            Hold::InFlight => true,
            Hold::Watching { file, .. }
            | Hold::Deferred { file, .. }
            | Hold::Ready { file, .. } => usable.contains(&file.root_id),
        });

        let mut actions = Vec::new();
        let mut room = self.max_in_flight.saturating_sub(self.in_flight());

        // Already proven stable: first in line, newest first.
        let mut ready: Vec<(std::cmp::Reverse<i64>, i64)> = self
            .held
            .iter()
            .filter_map(|(&id, hold)| match hold {
                Hold::Ready { file, .. } => Some((std::cmp::Reverse(file.mtime_ns), id)),
                _ => None,
            })
            .collect();
        ready.sort_unstable();
        for (_, id) in ready.into_iter().take(room) {
            if let Some(Hold::Ready { file, entry }) = self.held.insert(id, Hold::InFlight) {
                room -= 1;
                actions.push(Action::Extract { file, entry });
            }
        }

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
            actions.extend(self.look(&file, previous, now, &mut room));
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
                actions.extend(self.look(&file, None, now, &mut room));
            }
        }
        Ok(actions)
    }

    /// Stats one file and [`judge`](Self::judge)s it; one that cannot be
    /// stat'd is deleted or failed, and held until the writer reports back.
    fn look(
        &mut self,
        file: &PendingFile,
        previous_size: Option<u64>,
        now: Now,
        room: &mut usize,
    ) -> Option<Action> {
        match discovery::stat(&file.path) {
            Ok(entry) => self.judge(file, entry, previous_size, now, room),
            Err(e) => {
                self.held.insert(file.id, Hold::InFlight);
                Some(gone_or_failed(file.id, &e))
            }
        }
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
        let age = match now.unix_ns.saturating_sub(entry.mtime_ns) {
            past if past >= 0 => Duration::from_nanos(past as u64),
            // A hair ahead of our clock: written just now.
            ahead if -ahead < CLOCK_SLOP_NS => Duration::ZERO,
            // Far in the future (clock skew, or set that way): the time says
            // nothing about whether it is still being written, so the size
            // check decides instead of holding the file until then.
            _ => SETTLE,
        };
        // Younger than the settle window: look again once it would have
        // settled if left alone. Grown since the first look: wait longer.
        let wait = if age < SETTLE {
            Some((SETTLE - age).max(RECHECK))
        } else {
            previous_size
                .is_some_and(|s| s != entry.size)
                .then_some(DEFER)
        };
        if let Some(wait) = wait {
            self.held.insert(
                id,
                Hold::Deferred {
                    file: file.clone(),
                    until: now.at + wait,
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
        // Stable. No room yet: wait for it without looking again.
        if *room == 0 {
            self.held.insert(
                id,
                Hold::Ready {
                    file: file.clone(),
                    entry,
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

/// A file that could not be stat'd: deleted if it is gone, retried otherwise.
pub(crate) fn gone_or_failed(id: i64, e: &std::io::Error) -> Action {
    if e.kind() == std::io::ErrorKind::NotFound {
        Action::Delete(id)
    } else {
        Action::Fail {
            id,
            code: FileErrorCode::classify_io(e),
            message: e.to_string(),
        }
    }
}

#[cfg(test)]
mod tests;
