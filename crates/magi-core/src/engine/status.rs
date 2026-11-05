//! The status thread and what it reads [`IndexStatus`] from.

use std::sync::{Arc, Mutex};

use crossbeam_channel::{Receiver, RecvTimeoutError};
use rusqlite::Connection;

use crate::db::files::{self, FileState};
use crate::db::roots;
use crate::dto::{IndexState, IndexStatus, RootStatus};
use crate::error::Result;
use crate::index::resources::Pause;
use crate::index::writer::Stats;
use crate::watch::reconcile::ScanControl;

use super::{STATUS_INTERVAL, Subscribers, lock};

/// What [`IndexStatus`] is read from, shared by the handle and the status
/// thread. The reader connection only reads (WAL), so it never waits on the
/// writer.
#[derive(Clone)]
pub(super) struct StatusSource {
    pub(super) scan: Arc<ScanControl>,
    pub(super) reader: Arc<Mutex<Connection>>,
    pub(super) stats: Arc<Stats>,
    pub(super) pause: Arc<Pause>,
}

impl StatusSource {
    pub(super) fn read(&self) -> Result<IndexStatus> {
        let conn = lock(&self.reader);
        let counts = files::count_states(&conn)?;
        let count = |state: FileState| counts.get(&state).copied().unwrap_or(0);
        let queued = count(FileState::Pending) + count(FileState::Indexing);
        let paused_for = self.pause.reason();
        let state = if paused_for.is_some() {
            IndexState::Paused
        } else if self.stats.is_scanning() {
            IndexState::Scanning
        } else if queued > 0 {
            IndexState::Indexing
        } else {
            IndexState::Idle
        };
        // The last file started is current only while one is in the pipeline.
        let current_file = match state {
            IndexState::Indexing if count(FileState::Indexing) > 0 => self.stats.current_file(),
            _ => None,
        };
        Ok(IndexStatus {
            state,
            paused_for,
            scanned: if state == IndexState::Scanning {
                self.scan.seen()
            } else {
                0
            },
            queued,
            indexed: count(FileState::Indexed),
            skipped: count(FileState::Skipped),
            errors: count(FileState::Error),
            current_file: current_file.map(|p| p.to_string_lossy().into_owned()),
            roots: root_statuses(&conn)?,
        })
    }
}

/// Every root, with its indexed-file count.
pub(crate) fn root_statuses(conn: &Connection) -> Result<Vec<RootStatus>> {
    let counts = files::count_indexed_by_root(conn)?;
    Ok(roots::list(conn)?
        .into_iter()
        .map(|root| RootStatus::new(root, &counts))
        .collect())
}

/// Sends the status to every subscriber whenever it changes. The database is
/// read only after the writer applied something, or the pause or scan state
/// flipped, so an idle engine costs a few atomic loads twice a second.
pub(super) fn status_thread(
    source: &StatusSource,
    subscribers: &Subscribers,
    closed: &Receiver<()>,
) {
    let mut seen = None;
    let mut last = None;
    while let Err(RecvTimeoutError::Timeout) = closed.recv_timeout(STATUS_INTERVAL) {
        let now = (
            source.stats.generation(),
            source.pause.reason(),
            source.stats.is_scanning(),
            source.scan.seen(),
        );
        if seen == Some(now) {
            continue;
        }
        seen = Some(now);
        match source.read() {
            Ok(status) if last.as_ref() != Some(&status) => {
                lock(subscribers).retain(|s| s.send(status.clone()).is_ok());
                last = Some(status);
            }
            Ok(_) => {}
            Err(e) => tracing::warn!(error = %e, "could not read the index status"),
        }
    }
}
