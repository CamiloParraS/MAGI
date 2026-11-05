//! The pipeline threads: scheduler, extract workers and the embed worker.

use std::collections::HashMap;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, PoisonError};
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Sender, select};

use crate::db::roots;
use crate::dto::FileErrorCode;
use crate::index::lifecycle::{self, FileStep};
use crate::index::pipeline::{
    EMBED_BATCH, Extracted, Fresh, IndexContext, Job, embed_group, prepare,
};
use crate::index::resources::Pause;
use crate::index::scheduler::{Action, Now, Scheduler};
use crate::index::writer::{SharedOptions, Stats, WriteJob};
use crate::platform::{Os, ThreadPriority};

use super::{MAX_YIELD, POLL_INTERVAL};

/// Counts searches waiting for the models, so the embed worker can stand aside
/// between batches.
#[derive(Clone, Default)]
pub struct SearchPending(Arc<AtomicUsize>);

/// A search in progress; indexing yields until it is dropped.
pub struct SearchGuard(Arc<AtomicUsize>);

impl SearchPending {
    pub fn guard(&self) -> SearchGuard {
        self.0.fetch_add(1, Ordering::SeqCst);
        SearchGuard(self.0.clone())
    }

    fn wait_clear(&self) {
        let start = Instant::now();
        while self.0.load(Ordering::SeqCst) > 0 && start.elapsed() < MAX_YIELD {
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}

impl Drop for SearchGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

pub(super) fn scheduler_thread(
    conn: &rusqlite::Connection,
    mut scheduler: Scheduler,
    stop: &AtomicBool,
    pause: &Pause,
    (done, wake): (&Receiver<i64>, &Receiver<()>),
    write_tx: &Sender<WriteJob>,
    extract_tx: &Sender<Job>,
) {
    while !stop.load(Ordering::SeqCst) {
        while let Ok(id) = done.try_recv() {
            scheduler.finished(id);
        }
        if !pause.any() {
            match scheduler.poll(conn, Now::current()) {
                Ok(actions) => dispatch(conn, actions, write_tx, extract_tx),
                Err(e) => tracing::error!(error = %e, "scheduler poll failed"),
            }
        }
        select! {
            recv(done) -> id => if let Ok(id) = id { scheduler.finished(id) },
            recv(wake) -> _ => {},
            default(POLL_INTERVAL) => {},
        }
    }
}

/// Turns the scheduler's decisions into writer jobs and extract jobs.
fn dispatch(
    conn: &rusqlite::Connection,
    actions: Vec<Action>,
    write_tx: &Sender<WriteJob>,
    extract_tx: &Sender<Job>,
) {
    let roots: HashMap<i64, PathBuf> = match roots::list(conn) {
        Ok(list) => list.into_iter().map(|r| (r.id, r.path)).collect(),
        Err(e) => {
            tracing::error!(error = %e, "could not list roots");
            return;
        }
    };
    for action in actions {
        let (step, job) = lifecycle::begin(conn, action, &roots);
        // Ahead of the job on the writer's one channel, so its result finds
        // the row `indexing`.
        let _ = write_tx.send(step.into());
        if let Some(job) = job {
            let _ = extract_tx.send(job);
        }
    }
}

pub(super) fn extract_worker(
    ctx: &IndexContext,
    options: &SharedOptions,
    stats: &Stats,
    stop: &AtomicBool,
    jobs: Receiver<Job>,
    embed_tx: &Sender<(Job, Fresh)>,
    write_tx: &Sender<WriteJob>,
) {
    Os.lower_current_thread();
    for job in jobs {
        if stop.load(Ordering::SeqCst) {
            continue;
        }
        stats.set_current_file(&job.entry.path);
        let current = options
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        let next = match catch_unwind(AssertUnwindSafe(|| prepare(ctx, &current, &job))) {
            Ok(Extracted::Fresh(fresh)) => {
                let _ = embed_tx.send((job, *fresh));
                continue;
            }
            Ok(Extracted::Keep { state }) => FileStep::Keep {
                job: Box::new(job),
                state,
            },
            Ok(Extracted::Retry { code, message }) => FileStep::Retry {
                file_id: job.stored.id,
                code,
                message,
            },
            Err(_) => FileStep::retry(
                job.stored.id,
                FileErrorCode::Crashed,
                "extraction panicked".into(),
            ),
        };
        let _ = write_tx.send(next.into());
    }
}

pub(super) fn embed_worker(
    ctx: &IndexContext,
    stop: &AtomicBool,
    pending: &SearchPending,
    jobs: Receiver<(Job, Fresh)>,
    write_tx: &Sender<WriteJob>,
) {
    Os.lower_current_thread();
    for first in &jobs {
        if stop.load(Ordering::SeqCst) {
            continue;
        }
        // Whatever else is already waiting joins it, up to one model batch.
        let mut chunks = first.1.chunks_len();
        let mut group = vec![first];
        while chunks < EMBED_BATCH {
            let Ok(next) = jobs.try_recv() else { break };
            chunks += next.1.chunks_len();
            group.push(next);
        }
        let ids: Vec<i64> = group.iter().map(|(job, _)| job.stored.id).collect();
        let result = catch_unwind(AssertUnwindSafe(|| {
            embed_group(ctx, group, &|| pending.wait_clear())
        }));
        let Ok(results) = result else {
            for id in ids {
                let _ = write_tx.send(
                    FileStep::retry(id, FileErrorCode::Crashed, "embedding panicked".into()).into(),
                );
            }
            continue;
        };
        for (job, embedded) in results {
            let next = match embedded {
                Ok(embedded) => FileStep::Store {
                    job: Box::new(job),
                    embedded: Box::new(embedded),
                },
                Err(e) => {
                    FileStep::retry(job.stored.id, FileErrorCode::classify(&e), e.to_string())
                }
            };
            let _ = write_tx.send(next.into());
        }
    }
}
