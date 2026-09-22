//! Engine: owns threads, channels, and lifecycle (SPEC.md §5.3).
//!
//! ```text
//! scheduler ──► extract workers (N) ──► embed worker (1) ──► writer (1) ──► DB
//!     ▲  │            │  unchanged / retry ────────────────────▲              │
//!     │  └ deletes, state changes ────────────────────────────►│              │
//!     └───────────────────── done (file id) ◄───────────────────┴──────────────┘
//! ```
//!
//! The database's `pending` rows are the queue; the scheduler thread reads
//! them (on its own connection), waits for each file to stop changing, and
//! feeds the pipeline at a bounded rate. The writer is the only thread that
//! writes. No watcher yet (M5 Slice 4): [`EngineHandle::rescan`] is how new
//! and changed files are noticed after startup.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Sender, select};

use crate::config::Config;
use crate::db::{self, files, meta, roots};
use crate::embed::{ImageEmbedder, TextEmbedder};
use crate::error::{Error, Result};
use crate::index::pipeline::{
    Extracted, Fresh, IndexContext, IndexRootOptions, Job, embed, prepare,
};
use crate::index::scheduler::{Action, Now, Scheduler};
use crate::index::writer::{self, Stats, StatsSnapshot, WriteJob};
use crate::index::{ModelIds, requeue_on_model_change};
use crate::ocr::OcrEngine;
use crate::watch::reconcile::ScanSummary;

/// How often the scheduler looks at the queue when nothing wakes it sooner.
///
/// ponytail: fixed polling; back off when idle if idle CPU shows up in the
/// Slice 8 benchmark.
const POLL_INTERVAL: Duration = Duration::from_millis(250);
/// The longest an embed batch yields to a waiting search, so a leaked
/// [`SearchGuard`] cannot stall indexing for good.
const MAX_YIELD: Duration = Duration::from_secs(5);

/// The core engine that Tauri commands and the CLI talk to.
pub struct Engine;

impl Engine {
    pub fn ping(&self) -> &'static str {
        crate::ping()
    }

    /// Starts indexing (SPEC.md §5.4 startup steps 1, 2, 4 and 6): recovers rows
    /// left `indexing`, re-queues files made stale by a model change, probes and
    /// reconciles every enabled root, then runs the pipeline threads until
    /// [`EngineHandle::shutdown`].
    ///
    /// Blocks while the first reconciliation walk runs.
    /// ponytail: startup scan is synchronous; move it onto the writer without
    /// waiting if `start` blocks the UI on a very large root.
    pub fn start(
        config: &Config,
        db_path: &Path,
        text: Arc<dyn TextEmbedder>,
        image: Option<Arc<dyn ImageEmbedder>>,
        ocr: Arc<dyn OcrEngine>,
    ) -> Result<EngineHandle> {
        let options = Arc::new(IndexRootOptions::from_config(&config.indexing)?);
        let mut write_conn = db::open(db_path)?;
        files::reset_indexing_to_pending(&write_conn)?;
        requeue_on_model_change(
            &mut write_conn,
            &ModelIds {
                text: text.model_id(),
                image: image.as_deref().map(|e| e.model_id()),
                ocr: ocr.engine_id(),
            },
        )?;
        let scheduler_conn = db::open(db_path)?;

        let ctx = Arc::new(IndexContext {
            image_embedder: image,
            ocr,
            ..IndexContext::new(text)
        });
        let workers = match config.indexing.worker_threads {
            0 => (std::thread::available_parallelism().map_or(2, |n| n.get()) / 2).max(1),
            n => n as usize,
        };
        let max_in_flight = workers * 2 + 2;

        let (write_tx, write_rx) = crossbeam_channel::unbounded::<WriteJob>();
        let (done_tx, done_rx) = crossbeam_channel::unbounded::<i64>();
        let (wake_tx, wake_rx) = crossbeam_channel::bounded::<()>(1);
        let (extract_tx, extract_rx) = crossbeam_channel::bounded::<Job>(max_in_flight);
        let (embed_tx, embed_rx) = crossbeam_channel::bounded::<(Job, Fresh)>(workers);
        let stats = Arc::new(Stats::default());
        let stop = Arc::new(AtomicBool::new(false));
        let search_pending = SearchPending::default();

        let inner = Arc::new(Inner {
            stop: stop.clone(),
            threads: Mutex::new(Vec::new()),
            write_tx: write_tx.clone(),
            stats: stats.clone(),
            search_pending: search_pending.clone(),
        });
        let handle = EngineHandle { inner };

        handle.spawn("magi-writer", {
            let (options, stats, wake) = (options.clone(), stats.clone(), wake_tx.clone());
            move || writer::run(write_conn, options, write_rx, done_tx, wake, stats)
        })?;

        // Step 4: the reconciliation scan. Done before the workers exist, so
        // nothing is extracted while a moved file is still waiting to be matched.
        let scan = handle.rescan();
        scan.recv()
            .map_err(|_| Error::Engine("the writer stopped during startup".into()))??;

        handle.spawn("magi-embed", {
            let (ctx, stop, write_tx, pending) = (
                ctx.clone(),
                stop.clone(),
                write_tx.clone(),
                search_pending.clone(),
            );
            move || embed_worker(&ctx, &stop, &pending, embed_rx, &write_tx)
        })?;
        for n in 0..workers {
            let (ctx, options, stop) = (ctx.clone(), options.clone(), stop.clone());
            let (rx, embed_tx, write_tx) = (extract_rx.clone(), embed_tx.clone(), write_tx.clone());
            handle.spawn(&format!("magi-extract-{n}"), move || {
                extract_worker(&ctx, &options, &stop, rx, &embed_tx, &write_tx)
            })?;
        }
        drop((extract_rx, embed_tx));

        // Step 6: the queue, newest first, is the scheduler's job from here.
        handle.spawn("magi-scheduler", {
            let write_tx = write_tx.clone();
            move || {
                scheduler_thread(
                    &scheduler_conn,
                    Scheduler::new(max_in_flight),
                    &stop,
                    &done_rx,
                    &wake_rx,
                    &write_tx,
                    &extract_tx,
                )
            }
        })?;
        Ok(handle)
    }
}

/// Cheap, cloneable handle to a running [`Engine`]. The engine stops when the
/// last handle is dropped, or on [`shutdown`](Self::shutdown).
#[derive(Clone)]
pub struct EngineHandle {
    inner: Arc<Inner>,
}

struct Inner {
    stop: Arc<AtomicBool>,
    /// Spawn order: writer, embed, extract workers, scheduler.
    threads: Mutex<Vec<JoinHandle<()>>>,
    write_tx: Sender<WriteJob>,
    stats: Arc<Stats>,
    search_pending: SearchPending,
}

impl EngineHandle {
    fn spawn(&self, name: &str, f: impl FnOnce() + Send + 'static) -> Result<()> {
        let thread = std::thread::Builder::new()
            .name(name.to_string())
            .spawn(f)
            .map_err(|e| Error::Engine(format!("could not start thread {name}: {e}")))?;
        self.inner
            .threads
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(thread);
        Ok(())
    }

    /// What has been done since the engine started.
    pub fn stats(&self) -> StatsSnapshot {
        self.inner.stats.snapshot()
    }

    /// Walks every enabled root again: new and changed files become `pending`,
    /// vanished ones are deleted, moves are matched. The reply arrives when the
    /// scan is done; indexing then proceeds on its own. The watcher (M5 Slice 4)
    /// uses this for overflow and periodic safety nets.
    pub fn rescan(&self) -> Receiver<Result<ScanSummary>> {
        let (reply, result) = crossbeam_channel::bounded(1);
        // A stopped engine drops `reply`, which the caller sees as a
        // disconnected receiver.
        let _ = self.inner.write_tx.send(WriteJob::Reconcile(reply));
        result
    }

    /// Indexing yields to searches that hold a [`SearchGuard`] (SPEC.md §5.3,
    /// the priority lock). M6's search command takes one per query.
    pub fn search_pending(&self) -> SearchPending {
        self.inner.search_pending.clone()
    }

    /// Stops the threads and waits for them. Work still queued in the
    /// pipeline is dropped; its rows stay `indexing` and are recovered at the
    /// next start. Everything already handed to the writer is applied.
    pub fn shutdown(&self) {
        self.inner.shutdown();
    }
}

impl Inner {
    fn shutdown(&self) {
        self.stop.store(true, Ordering::SeqCst);
        let mut threads =
            std::mem::take(&mut *self.threads.lock().unwrap_or_else(PoisonError::into_inner));
        if threads.is_empty() {
            return;
        }
        let writer = threads.remove(0);
        // Front to back of the pipeline, so nothing is sent to a stopped stage.
        for thread in threads.into_iter().rev() {
            let _ = thread.join();
        }
        let _ = self.write_tx.send(WriteJob::Stop);
        let _ = writer.join();
    }
}

impl Drop for Inner {
    fn drop(&mut self) {
        self.shutdown();
    }
}

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

fn scheduler_thread(
    conn: &rusqlite::Connection,
    mut scheduler: Scheduler,
    stop: &AtomicBool,
    done: &Receiver<i64>,
    wake: &Receiver<()>,
    write_tx: &Sender<WriteJob>,
    extract_tx: &Sender<Job>,
) {
    while !stop.load(Ordering::SeqCst) {
        while let Ok(id) = done.try_recv() {
            scheduler.finished(id);
        }
        match scheduler.poll(conn, Now::current()) {
            Ok(actions) => dispatch(conn, actions, write_tx, extract_tx),
            Err(e) => tracing::error!(error = %e, "scheduler poll failed"),
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
    let roots: std::collections::HashMap<i64, PathBuf> = match roots::list(conn) {
        Ok(list) => list.into_iter().map(|r| (r.id, r.path)).collect(),
        Err(e) => {
            tracing::error!(error = %e, "could not list roots");
            return;
        }
    };
    let scan_id = meta::get(conn, "last_scan_id")
        .ok()
        .flatten()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    for action in actions {
        match action {
            Action::Delete(id) => {
                let _ = write_tx.send(WriteJob::Delete(id));
            }
            Action::Fail { id, message } => {
                let _ = write_tx.send(WriteJob::Retry {
                    file_id: id,
                    message,
                });
            }
            Action::Extract { file, entry } => {
                let stored = files::get_stored(conn, &entry.path).ok().flatten();
                let (Some(stored), Some(root_path)) = (stored, roots.get(&file.root_id)) else {
                    // The row or its root vanished since it was queued.
                    let _ = write_tx.send(WriteJob::Delete(file.id));
                    continue;
                };
                let _ = write_tx.send(WriteJob::MarkIndexing(file.id));
                let _ = extract_tx.send(Job {
                    root_id: file.root_id,
                    root_path: root_path.clone(),
                    entry,
                    scan_id,
                    stored: Some(stored),
                });
            }
        }
    }
}

fn file_id(job: &Job) -> Option<i64> {
    job.stored.as_ref().map(|s| s.id)
}

fn extract_worker(
    ctx: &IndexContext,
    options: &IndexRootOptions,
    stop: &AtomicBool,
    jobs: Receiver<Job>,
    embed_tx: &Sender<(Job, Fresh)>,
    write_tx: &Sender<WriteJob>,
) {
    for job in jobs {
        if stop.load(Ordering::SeqCst) {
            continue;
        }
        let extracted = catch_unwind(AssertUnwindSafe(|| prepare(ctx, options, &job)));
        let next = match extracted {
            Ok(Extracted::Keep { file_id, state }) => WriteJob::Keep {
                job: Box::new(job),
                file_id,
                state,
            },
            Ok(Extracted::Retry { file_id, message }) => WriteJob::Retry { file_id, message },
            Ok(Extracted::Fresh(fresh)) => {
                let _ = embed_tx.send((job, *fresh));
                continue;
            }
            Err(_) => match file_id(&job) {
                Some(file_id) => WriteJob::Retry {
                    file_id,
                    message: "extraction panicked".into(),
                },
                None => continue,
            },
        };
        let _ = write_tx.send(next);
    }
}

fn embed_worker(
    ctx: &IndexContext,
    stop: &AtomicBool,
    pending: &SearchPending,
    jobs: Receiver<(Job, Fresh)>,
    write_tx: &Sender<WriteJob>,
) {
    for (job, fresh) in jobs {
        if stop.load(Ordering::SeqCst) {
            continue;
        }
        let result = catch_unwind(AssertUnwindSafe(|| {
            embed(ctx, &job, fresh, &|| pending.wait_clear())
        }));
        let next = match result {
            Ok(Ok(embedded)) => WriteJob::Store {
                job: Box::new(job),
                embedded: Box::new(embedded),
            },
            Ok(Err(e)) => match file_id(&job) {
                Some(file_id) => WriteJob::Retry {
                    file_id,
                    message: e.to_string(),
                },
                None => continue,
            },
            Err(_) => match file_id(&job) {
                Some(file_id) => WriteJob::Retry {
                    file_id,
                    message: "embedding panicked".into(),
                },
                None => continue,
            },
        };
        let _ = write_tx.send(next);
    }
}
