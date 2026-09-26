//! A file's life in the index (SPEC.md §5.4): `pending -> indexing ->
//! indexed | skipped | error`, or deleted. Both drivers, the engine's threads
//! and the one-shot [`index_root`](super::pipeline::index_root), make every
//! state change through [`begin`] and [`apply`], so the rules live here once.

use std::collections::HashMap;
use std::path::PathBuf;

use rusqlite::Connection;

use crate::db::files::{self, FileState};
use crate::dto::FileErrorCode;
use crate::error::Result;

use super::pipeline::{Embedded, Job, Status, store_embedded, unix_now};
use super::scheduler::Action;

/// One state change for one file, applied by whoever writes ([`apply`]).
pub(crate) enum FileStep {
    /// Entered the pipeline (`pending -> indexing`).
    Start(i64),
    /// Fully processed: write the row, chunks and vectors.
    Store {
        job: Box<Job>,
        embedded: Box<Embedded>,
    },
    /// Unchanged: refresh size, mtime and scan id only.
    Keep { job: Box<Job>, state: FileState },
    /// Failed for a reason that may pass: back off and retry.
    Retry {
        file_id: i64,
        code: FileErrorCode,
        message: String,
    },
    /// Gone from disk.
    Delete(i64),
}

impl FileStep {
    pub(crate) fn retry(file_id: i64, code: FileErrorCode, message: String) -> Self {
        Self::Retry {
            file_id,
            code,
            message,
        }
    }

    pub(crate) fn file_id(&self) -> i64 {
        match self {
            Self::Start(id) | Self::Delete(id) | Self::Retry { file_id: id, .. } => *id,
            Self::Store { job, .. } | Self::Keep { job, .. } => job.stored.id,
        }
    }
}

/// The first step for a scheduler decision and, for a file to extract, its
/// job. The step is applied before the job's result, so the result finds the
/// row `indexing`. A row or root that vanished since it was queued is deleted.
pub(crate) fn begin(
    conn: &Connection,
    action: Action,
    roots: &HashMap<i64, PathBuf>,
) -> (FileStep, Option<Job>) {
    let (file, entry) = match action {
        Action::Delete(id) => return (FileStep::Delete(id), None),
        Action::Fail { id, code, message } => return (FileStep::retry(id, code, message), None),
        Action::Extract { file, entry } => (file, entry),
    };
    let stored = files::get_stored(conn, &entry.path)
        .inspect_err(|e| tracing::warn!(file_id = file.id, error = %e, "could not read file row"))
        .ok()
        .flatten();
    let (Some(stored), Some(root_path)) = (stored, roots.get(&file.root_id)) else {
        return (FileStep::Delete(file.id), None);
    };
    let job = Job {
        root_id: file.root_id,
        root_path: root_path.clone(),
        entry,
        stored,
    };
    (FileStep::Start(file.id), Some(job))
}

/// Applies one step. `None` when nothing is counted: a start, or a result that
/// arrived after its file was queued again or deleted.
pub(crate) fn apply(conn: &mut Connection, step: FileStep) -> Result<Option<Status>> {
    apply_as(conn, step, Writer::Shared)
}

/// [`apply`] for a caller that is the database's only writer and works one
/// file at a time (the one-shot run): nothing can queue a file again while it
/// is worked on, so no `indexing` mark is written or checked. That saves a
/// commit per file, about 5% of a cold run over small files.
pub(crate) fn apply_alone(conn: &mut Connection, step: FileStep) -> Result<Option<Status>> {
    apply_as(conn, step, Writer::Alone)
}

#[derive(Clone, Copy, PartialEq)]
enum Writer {
    /// The engine: the watcher may queue a file again while it is in flight.
    Shared,
    Alone,
}

fn apply_as(conn: &mut Connection, step: FileStep, writer: Writer) -> Result<Option<Status>> {
    let stale = |conn: &Connection, id| writer == Writer::Shared && superseded(conn, id);
    match step {
        FileStep::Start(id) => {
            if writer == Writer::Shared {
                files::mark_indexing(conn, id)?;
            }
            Ok(None)
        }
        FileStep::Delete(id) => {
            files::delete_file(conn, id)?;
            Ok(Some(Status::Removed))
        }
        FileStep::Retry {
            file_id,
            code,
            message,
        } => retry(conn, file_id, code, &message).map(Some),
        FileStep::Keep { job, state } => {
            if stale(conn, job.stored.id) {
                return Ok(None);
            }
            files::touch_unchanged(
                conn,
                job.stored.id,
                job.entry.size,
                job.entry.mtime_ns,
                state,
            )?;
            Ok(Some(Status::Unchanged))
        }
        FileStep::Store { job, embedded } => {
            let id = job.stored.id;
            if stale(conn, id) {
                return Ok(None);
            }
            match store_embedded(conn, &job, *embedded) {
                Ok(status) => Ok(Some(status)),
                Err(e) => {
                    // Leave the file retryable rather than stuck in `indexing`.
                    let _ = retry(conn, id, FileErrorCode::classify(&e), &e.to_string());
                    Err(e)
                }
            }
        }
    }
}

/// Whether a result for this file is out of date: it was queued again (changed
/// while in the pipeline), deleted, or its root removed. Storing it anyway
/// would bring back a deleted row. Only a row still `indexing` takes it.
///
/// `Retry` and `Delete` are not checked: the scheduler sends them for rows it
/// never started, which are still `pending`.
fn superseded(conn: &Connection, file_id: i64) -> bool {
    files::state_of(conn, file_id).is_ok_and(|state| state != Some(FileState::Indexing))
}

fn retry(conn: &Connection, file_id: i64, code: FileErrorCode, message: &str) -> Result<Status> {
    let failure = if code == FileErrorCode::Locked {
        files::record_locked(conn, file_id, code, message, unix_now())?
    } else {
        files::record_failure(conn, file_id, code, message, unix_now())?
    };
    Ok(match failure {
        files::Failure::Retry { .. } => Status::Retried,
        files::Failure::GaveUp { .. } => Status::Errored,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{self, files::PendingFile};
    use crate::discovery;
    use std::path::Path;

    struct Fixture {
        _db: tempfile::TempDir,
        dir: tempfile::TempDir,
        conn: Connection,
        roots: HashMap<i64, PathBuf>,
    }

    impl Fixture {
        fn new() -> Self {
            let db_dir = tempfile::tempdir().unwrap();
            let conn = db::open(&db_dir.path().join("magi.db")).unwrap();
            let dir = tempfile::tempdir().unwrap();
            let root = db::roots::add(&conn, dir.path()).unwrap();
            Self {
                _db: db_dir,
                roots: HashMap::from([(root.id, root.path)]),
                dir,
                conn,
            }
        }

        /// A file on disk queued as a reconciliation would, as the scheduler
        /// hands it out once it is stable.
        fn stable(&self, name: &str) -> Action {
            let path = self.dir.path().join(name);
            std::fs::write(&path, "hello").unwrap();
            let entry = discovery::stat(&path).unwrap();
            let root_id = *self.roots.keys().next().unwrap();
            files::insert_pending(
                &self.conn,
                root_id,
                &path,
                Path::new(name),
                entry.size,
                entry.mtime_ns,
                1,
            )
            .unwrap();
            let id = files::get_stored(&self.conn, &path).unwrap().unwrap().id;
            Action::Extract {
                file: PendingFile {
                    id,
                    root_id,
                    path,
                    size: entry.size,
                    mtime_ns: entry.mtime_ns,
                },
                entry,
            }
        }

        fn state(&self, id: i64) -> Option<FileState> {
            files::state_of(&self.conn, id).unwrap()
        }
    }

    fn keep(job: &Job) -> FileStep {
        FileStep::Keep {
            job: Box::new(job.clone()),
            state: FileState::Indexed,
        }
    }

    #[test]
    fn a_stable_file_starts_indexing() {
        let mut f = Fixture::new();
        let (step, job) = begin(&f.conn, f.stable("a.txt"), &f.roots);
        let job = job.expect("a job to extract");
        assert!(matches!(step, FileStep::Start(id) if id == job.stored.id));
        assert!(apply(&mut f.conn, step).unwrap().is_none(), "not counted");
        assert_eq!(f.state(job.stored.id), Some(FileState::Indexing));
    }

    /// The row went between the scheduler's read and now: delete, never leave
    /// it `pending` for the queue to hand out again.
    #[test]
    fn a_file_whose_row_vanished_is_deleted_not_started() {
        let mut f = Fixture::new();
        let action = f.stable("a.txt");
        let Action::Extract { file, .. } = &action else {
            unreachable!()
        };
        let id = file.id;
        files::delete_file(&mut f.conn, id).unwrap();
        let (step, job) = begin(&f.conn, action, &f.roots);
        assert!(job.is_none());
        assert!(matches!(step, FileStep::Delete(d) if d == id));
        assert!(matches!(
            apply(&mut f.conn, step).unwrap(),
            Some(Status::Removed)
        ));
    }

    #[test]
    fn gone_and_failed_stats_pass_straight_through() {
        let f = Fixture::new();
        let (step, job) = begin(&f.conn, Action::Delete(3), &f.roots);
        assert!(job.is_none() && matches!(step, FileStep::Delete(3)));
        let fail = Action::Fail {
            id: 4,
            code: FileErrorCode::ReadFailed,
            message: "busy".into(),
        };
        let (step, job) = begin(&f.conn, fail, &f.roots);
        assert!(job.is_none());
        assert!(matches!(
            step,
            FileStep::Retry {
                file_id: 4,
                code: FileErrorCode::ReadFailed,
                ..
            }
        ));
    }

    #[test]
    fn only_a_row_still_indexing_takes_a_result() {
        let mut f = Fixture::new();
        let (start, job) = begin(&f.conn, f.stable("a.txt"), &f.roots);
        let job = job.unwrap();
        let id = job.stored.id;
        apply(&mut f.conn, start).unwrap();
        assert!(matches!(
            apply(&mut f.conn, keep(&job)).unwrap(),
            Some(Status::Unchanged)
        ));
        assert_eq!(f.state(id), Some(FileState::Indexed));

        files::mark_pending(&f.conn, id).unwrap();
        assert!(
            apply(&mut f.conn, keep(&job)).unwrap().is_none(),
            "changed again while in the pipeline"
        );
        assert_eq!(f.state(id), Some(FileState::Pending));

        files::delete_file(&mut f.conn, id).unwrap();
        assert!(
            apply(&mut f.conn, keep(&job)).unwrap().is_none(),
            "deleted, or its root removed"
        );
        assert_eq!(f.state(id), None);
    }

    /// Only scans say when a row was last seen: storing a result must not move
    /// it, or a result dispatched before a later scan would set it back.
    #[test]
    fn a_result_leaves_seen_scan_id_to_the_scans() {
        let mut f = Fixture::new();
        let (start, job) = begin(&f.conn, f.stable("a.txt"), &f.roots);
        let job = job.unwrap();
        apply(&mut f.conn, start).unwrap();
        files::mark_seen(&f.conn, job.stored.id, 5).unwrap();
        apply(&mut f.conn, keep(&job)).unwrap();
        let seen: i64 = f
            .conn
            .query_row(
                "SELECT seen_scan_id FROM files WHERE id = ?1",
                [job.stored.id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(seen, 5);
    }

    /// A scan matched the file's move while it was in the pipeline: the result
    /// was read at the old path and must not bring that path back.
    #[test]
    fn a_result_for_a_file_moved_mid_flight_is_dropped() {
        let mut f = Fixture::new();
        let (start, job) = begin(&f.conn, f.stable("old.txt"), &f.roots);
        let job = job.unwrap();
        apply(&mut f.conn, start).unwrap();
        let new_path = f.dir.path().join("new.txt");
        let to = files::MoveTarget {
            path: &new_path,
            rel_path: Path::new("new.txt"),
            size: job.entry.size,
            mtime_ns: job.entry.mtime_ns,
        };
        files::rename_file_to(&f.conn, job.stored.id, job.root_id, to, 2).unwrap();

        assert!(apply(&mut f.conn, keep(&job)).unwrap().is_none());
        assert_eq!(
            f.state(job.stored.id),
            Some(FileState::Pending),
            "queued at its new path"
        );
        assert!(
            files::get_stored(&f.conn, &job.entry.path)
                .unwrap()
                .is_none()
        );
    }

    /// The one-shot run is the only writer: nothing can queue a file again
    /// mid-flight, so it writes no `indexing` mark and its results still land.
    #[test]
    fn a_lone_writer_skips_the_indexing_mark() {
        let mut f = Fixture::new();
        let (start, job) = begin(&f.conn, f.stable("a.txt"), &f.roots);
        let job = job.unwrap();
        assert!(apply_alone(&mut f.conn, start).unwrap().is_none());
        assert_eq!(f.state(job.stored.id), Some(FileState::Pending));
        assert!(matches!(
            apply_alone(&mut f.conn, keep(&job)).unwrap(),
            Some(Status::Unchanged)
        ));
        assert_eq!(f.state(job.stored.id), Some(FileState::Indexed));
    }

    /// A stat failure hits a row that never left `pending`: it must still be
    /// recorded, or the file would be retried with no delay.
    #[test]
    fn a_retry_applies_to_a_pending_row() {
        let mut f = Fixture::new();
        let Action::Extract { file, .. } = f.stable("a.txt") else {
            unreachable!()
        };
        let step = FileStep::retry(file.id, FileErrorCode::ReadFailed, "locked".into());
        assert_eq!(step.file_id(), file.id);
        assert!(matches!(
            apply(&mut f.conn, step).unwrap(),
            Some(Status::Retried)
        ));
    }
}
