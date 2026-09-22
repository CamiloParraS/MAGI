//! The single writer: the only thread that writes to the database (SPEC.md
//! §5.3). Every state change, upsert, delete and reconciliation arrives as a
//! [`WriteJob`] on one channel and is applied in order, one transaction per
//! file, so writes never contend and a crash leaves at most one file
//! half-done. Search reads on other connections (WAL).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError, RwLock};
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Sender};
use rusqlite::Connection;

use crate::db::files;
use crate::error::Result;
use crate::index::pipeline::{
    Embedded, IndexRootOptions, Job, Status, store_embedded, store_keep, store_retry,
};
use crate::watch::reconcile::{
    Held, ScanSummary, reconcile_all, reconcile_roots, recover, scan_paths, settle_held,
};

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
    Keep { job: Box<Job>, state: String },
    /// Failed for a reason that may pass: back off and retry.
    Retry {
        file_id: i64,
        message: String,
        locked: bool,
    },
    /// Gone from disk.
    Delete(i64),
    /// Walk every enabled root and settle deletions and moves; the result goes
    /// to the sender, if anyone is waiting for it.
    Reconcile(Option<Sender<Result<ScanSummary>>>),
    /// Walk just this root (added or enabled again), like `Reconcile`.
    ReconcileRoot(i64),
    /// Reconcile if a root that was unreadable or missing is back.
    Reprobe,
    /// The watcher saw these paths change: reconcile just them.
    Paths(Vec<PathBuf>),
    /// Any other write (root management, pause, retry), in order with the rest.
    Exec(Box<dyn FnOnce(&mut Connection) + Send>),
    /// Finish everything queued before this, then stop.
    Stop,
}

impl WriteJob {
    /// A failure that may pass, not caused by a lock.
    pub(crate) fn retry(file_id: i64, message: String) -> Self {
        Self::Retry {
            file_id,
            message,
            locked: false,
        }
    }
}

/// What the engine has done since it started.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StatsSnapshot {
    pub indexed: u64,
    pub skipped: u64,
    pub errored: u64,
    pub unchanged: u64,
    pub retried: u64,
    pub removed: u64,
}

/// Live progress: the [`StatsSnapshot`] and whether a scan is running, kept by
/// the writer, and the file most recently started, set by the extract workers.
#[derive(Default)]
pub struct Stats {
    counts: Mutex<StatsSnapshot>,
    /// Bumped after every writer job, so a status watcher looks again only then.
    generation: AtomicU64,
    scanning: AtomicBool,
    current_file: Mutex<Option<PathBuf>>,
}

impl Stats {
    pub fn snapshot(&self) -> StatsSnapshot {
        *self.counts.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub(crate) fn generation(&self) -> u64 {
        self.generation.load(Ordering::SeqCst)
    }

    pub(crate) fn is_scanning(&self) -> bool {
        self.scanning.load(Ordering::SeqCst)
    }

    pub(crate) fn current_file(&self) -> Option<PathBuf> {
        self.current_file
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    pub(crate) fn set_current_file(&self, path: &Path) {
        *self
            .current_file
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(path.to_path_buf());
    }

    fn scanning<T>(&self, scan: impl FnOnce() -> T) -> T {
        self.scanning.store(true, Ordering::SeqCst);
        let result = scan();
        self.scanning.store(false, Ordering::SeqCst);
        result
    }

    fn update(&self, f: impl FnOnce(&mut StatsSnapshot)) {
        f(&mut self.counts.lock().unwrap_or_else(PoisonError::into_inner));
    }

    fn count(&self, status: &Status) {
        self.update(|s| {
            *match status {
                Status::Indexed => &mut s.indexed,
                Status::Skipped => &mut s.skipped,
                Status::Errored => &mut s.errored,
                Status::Unchanged => &mut s.unchanged,
                Status::Retried => &mut s.retried,
            } += 1;
        });
    }

    fn removed(&self, n: u32) {
        self.update(|s| s.removed += u64::from(n));
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
        // The previous job is applied.
        stats.generation.fetch_add(1, Ordering::SeqCst);
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
            WriteJob::Exec(write) => {
                write(&mut conn);
                let _ = wake.try_send(());
                continue;
            }
            WriteJob::Reconcile(reply) => {
                let summary = stats.scanning(|| reconcile_all(&mut conn, &current(&options)));
                if let Ok(s) = &summary {
                    stats.removed(s.removed);
                }
                if let Some(reply) = reply {
                    let _ = reply.send(summary);
                }
                let _ = wake.try_send(());
                continue;
            }
            WriteJob::ReconcileRoot(id) => {
                let scan = stats.scanning(|| reconcile_roots(&mut conn, &current(&options), &[id]));
                match scan {
                    Ok(s) => stats.removed(s.removed),
                    Err(e) => tracing::error!(root_id = id, error = %e, "could not scan root"),
                }
                let _ = wake.try_send(());
                continue;
            }
            WriteJob::Reprobe => {
                match recover(&mut conn, &current(&options)) {
                    Ok(Some(s)) => {
                        tracing::info!("a root is readable again; reconciled");
                        stats.removed(s.removed);
                        let _ = wake.try_send(());
                    }
                    Ok(None) => {}
                    Err(e) => tracing::error!(error = %e, "could not re-probe roots"),
                }
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
                    Ok(()) => stats.removed(1),
                    Err(e) => tracing::error!(file_id = id, error = %e, "could not delete file"),
                }
                id
            }
            WriteJob::Retry {
                file_id,
                message,
                locked,
            } => {
                apply(
                    &stats,
                    file_id,
                    store_retry(&conn, file_id, &message, locked),
                );
                file_id
            }
            WriteJob::Keep { job, state } => {
                let id = job.stored.id;
                if !superseded(&conn, id) {
                    apply(&stats, id, store_keep(&conn, &job, &state));
                }
                id
            }
            WriteJob::Store { job, embedded } => {
                let id = job.stored.id;
                if !superseded(&conn, id) {
                    let result = store_embedded(&mut conn, &job, *embedded);
                    if let Err(e) = &result {
                        // Leave the file retryable rather than stuck in `indexing`.
                        tracing::error!(file_id = id, error = %e, "could not store file");
                        let _ = store_retry(&conn, id, &e.to_string(), false);
                    }
                    apply(&stats, id, result);
                }
                id
            }
        };
        let _ = done.send(id);
    }
}

fn settle(conn: &mut Connection, held: &mut Vec<Held>, stats: &Stats) {
    match settle_held(conn, held, Instant::now()) {
        Ok(removed) => stats.removed(removed),
        Err(e) => tracing::error!(error = %e, "could not settle removed files"),
    }
}

/// The file was changed again while the pipeline was working on it (the watcher
/// put it back to `pending`): what was extracted is already stale, so it is
/// dropped and the file is picked up again.
/// Whether a result for this file is out of date: it was queued again (changed
/// while in the pipeline), deleted, or its root removed. Storing it anyway
/// would bring back a deleted row. Only a row still `indexing` takes it.
fn superseded(conn: &Connection, file_id: i64) -> bool {
    files::state_of(conn, file_id).is_ok_and(|state| state.as_deref() != Some("indexing"))
}

fn apply(stats: &Stats, file_id: i64, result: Result<Status>) {
    match result {
        Ok(status) => stats.count(&status),
        Err(e) => tracing::error!(file_id, error = %e, "write failed"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;
    use std::path::Path;

    #[test]
    fn only_a_row_still_indexing_takes_a_result() {
        let dir = tempfile::tempdir().unwrap();
        let mut conn = db::open(&dir.path().join("magi.db")).unwrap();
        let root = db::roots::add(&conn, dir.path()).unwrap();
        let path = root.path.join("a.txt");
        files::insert_pending(&conn, root.id, &path, Path::new("a.txt"), 1, 1, 1).unwrap();
        let id = files::get_stored(&conn, &path).unwrap().unwrap().id;

        files::mark_indexing(&conn, id).unwrap();
        assert!(!superseded(&conn, id));
        files::mark_pending(&conn, id).unwrap();
        assert!(superseded(&conn, id), "changed again while in the pipeline");
        files::delete_file(&mut conn, id).unwrap();
        assert!(superseded(&conn, id), "deleted, or its root removed");
    }
}
