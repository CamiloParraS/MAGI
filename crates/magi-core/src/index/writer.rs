//! The single writer: the only thread that writes to the database (SPEC.md
//! §5.3). Every state change, upsert, delete and reconciliation arrives as a
//! [`WriteJob`] on one channel and is applied in order, one transaction per
//! file, so writes never contend and a crash leaves at most one file
//! half-done. Search reads on other connections (WAL).

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, PoisonError, RwLock};
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Sender};
use rusqlite::Connection;

use crate::db::files;
use crate::error::Result;
use crate::index::pipeline::{
    Embedded, IndexRootOptions, Job, Status, store_embedded, store_keep, store_retry,
};
use crate::watch::reconcile::{Held, ScanSummary, reconcile_all, scan_paths, settle_held};

/// The indexing options in force, replaceable while running (an exclusion
/// changed in settings). Each job reads the current ones.
pub(crate) type SharedOptions = Arc<RwLock<Arc<IndexRootOptions>>>;

fn current(options: &SharedOptions) -> Arc<IndexRootOptions> {
    options
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .clone()
}

pub(crate) enum WriteJob {
    /// The file entered the pipeline (`pending -> indexing`).
    MarkIndexing(i64),
    /// Fully processed: write the row, chunks and vectors.
    Store {
        job: Box<Job>,
        embedded: Box<Embedded>,
    },
    /// Unchanged: refresh size, mtime and scan id only.
    Keep {
        job: Box<Job>,
        file_id: i64,
        state: String,
    },
    /// Failed for a reason that may pass: back off and retry.
    Retry { file_id: i64, message: String },
    /// Gone from disk.
    Delete(i64),
    /// Walk every enabled root and settle deletions and moves.
    Reconcile(Sender<Result<ScanSummary>>),
    /// The watcher saw these paths change: reconcile just them.
    Paths(Vec<PathBuf>),
    /// Finish everything queued before this, then stop.
    Stop,
}

/// What the engine has done since it started.
#[derive(Default)]
pub struct Stats {
    pub indexed: AtomicU64,
    pub skipped: AtomicU64,
    pub errored: AtomicU64,
    pub unchanged: AtomicU64,
    pub retried: AtomicU64,
    pub removed: AtomicU64,
}

/// A point-in-time copy of [`Stats`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StatsSnapshot {
    pub indexed: u64,
    pub skipped: u64,
    pub errored: u64,
    pub unchanged: u64,
    pub retried: u64,
    pub removed: u64,
}

impl Stats {
    pub fn snapshot(&self) -> StatsSnapshot {
        let get = |a: &AtomicU64| a.load(Ordering::Relaxed);
        StatsSnapshot {
            indexed: get(&self.indexed),
            skipped: get(&self.skipped),
            errored: get(&self.errored),
            unchanged: get(&self.unchanged),
            retried: get(&self.retried),
            removed: get(&self.removed),
        }
    }

    fn count(&self, status: &Status) {
        let counter = match status {
            Status::Indexed => &self.indexed,
            Status::Skipped => &self.skipped,
            Status::Errored => &self.errored,
            Status::Unchanged => &self.unchanged,
            Status::Retried => &self.retried,
        };
        counter.fetch_add(1, Ordering::Relaxed);
    }
}

/// How long a file the watcher saw disappear is kept before it is deleted, so
/// the other half of a move (a creation reported in a later batch, possibly by
/// another root's watcher) can still claim it and spare it a re-embed.
const HOLD: Duration = Duration::from_secs(5);
/// How often held files are looked at when no job arrives.
const HOLD_TICK: Duration = Duration::from_millis(500);

/// Runs until [`WriteJob::Stop`]. `done` gets the id of every file the pipeline
/// held, once its job has been applied (or has failed), so the scheduler can
/// release it; `wake` nudges the scheduler after a reconciliation.
pub(crate) fn run(
    mut conn: Connection,
    options: SharedOptions,
    jobs: Receiver<WriteJob>,
    done: Sender<i64>,
    wake: Sender<()>,
    stats: Arc<Stats>,
) {
    let mut held: Vec<Held> = Vec::new();
    loop {
        let job = if held.is_empty() {
            match jobs.recv() {
                Ok(job) => job,
                Err(_) => return,
            }
        } else {
            match jobs.recv_timeout(HOLD_TICK) {
                Ok(job) => job,
                Err(crossbeam_channel::RecvTimeoutError::Timeout) => {
                    settle(&mut conn, &mut held, &stats);
                    continue;
                }
                Err(crossbeam_channel::RecvTimeoutError::Disconnected) => return,
            }
        };
        let id = match job {
            WriteJob::Stop => return,
            WriteJob::Reconcile(reply) => {
                let summary = reconcile_all(&mut conn, &current(&options));
                if let Ok(s) = &summary {
                    stats
                        .removed
                        .fetch_add(u64::from(s.removed), Ordering::Relaxed);
                }
                let _ = reply.send(summary);
                let _ = wake.try_send(());
                continue;
            }
            WriteJob::Paths(paths) => {
                match scan_paths(&mut conn, &current(&options), &paths) {
                    Ok(scan) => {
                        for id in scan.unseen {
                            if !held.iter().any(|h| h.id == id) {
                                held.push(Held {
                                    id,
                                    scan_id: scan.scan_id,
                                    until: Instant::now() + HOLD,
                                });
                            }
                        }
                        settle(&mut conn, &mut held, &stats);
                    }
                    Err(e) => tracing::error!(error = %e, "could not reconcile changed paths"),
                }
                let _ = wake.try_send(());
                continue;
            }
            WriteJob::MarkIndexing(id) => {
                if let Err(e) = files::mark_indexing(&conn, id) {
                    tracing::error!(file_id = id, error = %e, "could not mark file indexing");
                }
                continue;
            }
            WriteJob::Delete(id) => {
                match files::delete_file(&mut conn, id) {
                    Ok(_) => {
                        stats.removed.fetch_add(1, Ordering::Relaxed);
                    }
                    Err(e) => tracing::error!(file_id = id, error = %e, "could not delete file"),
                }
                id
            }
            WriteJob::Retry { file_id, message } => {
                apply(&stats, file_id, store_retry(&conn, file_id, &message));
                file_id
            }
            WriteJob::Keep {
                job,
                file_id,
                state,
            } => {
                if requeued(&conn, file_id) {
                    let _ = done.send(file_id);
                    continue;
                }
                apply(&stats, file_id, store_keep(&conn, &job, file_id, &state));
                file_id
            }
            WriteJob::Store { job, embedded } => {
                let file_id = job.stored.as_ref().map(|s| s.id);
                if file_id.is_some_and(|id| requeued(&conn, id)) {
                    let _ = done.send(file_id.unwrap_or_default());
                    continue;
                }
                let result = store_embedded(&mut conn, &job, *embedded);
                let id = file_id.unwrap_or_default();
                if let Err(e) = &result
                    && file_id.is_some()
                {
                    // Leave the file retryable rather than stuck in `indexing`.
                    tracing::error!(file_id = id, error = %e, "could not store file");
                    let _ = store_retry(&conn, id, &e.to_string());
                }
                apply(&stats, id, result);
                id
            }
        };
        let _ = done.send(id);
    }
}

fn settle(conn: &mut Connection, held: &mut Vec<Held>, stats: &Stats) {
    match settle_held(conn, held, Instant::now()) {
        Ok(removed) => {
            stats
                .removed
                .fetch_add(u64::from(removed), Ordering::Relaxed);
        }
        Err(e) => tracing::error!(error = %e, "could not settle removed files"),
    }
}

/// The file was changed again while the pipeline was working on it (the watcher
/// put it back to `pending`): what was extracted is already stale, so it is
/// dropped and the file is picked up again.
fn requeued(conn: &Connection, file_id: i64) -> bool {
    files::state_of(conn, file_id).ok().flatten().as_deref() == Some("pending")
}

fn apply(stats: &Stats, file_id: i64, result: Result<Status>) {
    match result {
        Ok(status) => stats.count(&status),
        Err(e) => tracing::error!(file_id, error = %e, "write failed"),
    }
}
