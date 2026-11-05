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
//! writes. Per-root watchers and the ticker (`watch/`) feed it changes; the
//! extract and embed threads run at low OS priority. The monitor
//! (`index::resources`) pauses the scheduler on low memory or battery and
//! unloads idle models. The status thread tells subscribers what changed.
//! Root management, pause and retry run on the writer too
//! ([`WriteJob::Exec`]).

mod root_ops;
mod status;
mod workers;

pub(crate) use status::root_statuses;
use status::{StatusSource, status_thread};
pub use workers::{SearchGuard, SearchPending};
use workers::{embed_worker, extract_worker, scheduler_thread};

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError, RwLock};
use std::thread::JoinHandle;
use std::time::Duration;

use crossbeam_channel::{Receiver, RecvTimeoutError, Sender};
use rusqlite::Connection;

use crate::config::{Config, IndexingConfig};
use crate::db::roots::Health;
use crate::db::{self, files, meta, roots};
use crate::dto::IndexStatus;
use crate::error::{Error, Result};
use crate::features::Components;
use crate::index::pipeline::{Fresh, IndexContext, IndexRootOptions, Job};
use crate::index::resources::{self, Pause};
use crate::index::scheduler::Scheduler;
use crate::index::writer::{self, SharedOptions, Stats, StatsSnapshot, WriteJob};
use crate::index::{ModelIds, requeue_missing, requeue_on_model_change};
use crate::platform::{FsProbe, PermissionProbe, RootAccess};
use crate::watch::reconcile::{ScanControl, ScanSummary};
use crate::watch::watcher::Watchers;
use crate::watch::{poller, watcher};

/// How often the scheduler looks at the queue when nothing wakes it sooner.
///
/// ponytail: fixed polling; back off when idle if idle CPU shows up in a
/// benchmark.
const POLL_INTERVAL: Duration = Duration::from_millis(250);
/// The longest an embed batch yields to a waiting search, so a leaked
/// [`SearchGuard`] cannot stall indexing for good.
const MAX_YIELD: Duration = Duration::from_secs(5);
/// How often the status thread checks whether anything changed.
const STATUS_INTERVAL: Duration = Duration::from_millis(500);

/// The core engine that the desktop host and the CLI talk to.
pub struct Engine;

impl Engine {
    /// Starts indexing (SPEC.md §5.4 startup steps 1, 2, 4 and 6): recovers rows
    /// left `indexing`, re-queues files made stale by a model change, probes and
    /// reconciles every enabled root, then runs the pipeline threads until
    /// [`EngineHandle::shutdown`]. `components` are the running search
    /// features (ADR-0010); a missing one is skipped by indexing.
    ///
    /// Returns before the first reconciliation walk ends; the status reads
    /// `Scanning` meanwhile and the scheduler starts when the walk is done.
    pub fn start(config: &Config, db_path: &Path, components: Components) -> Result<EngineHandle> {
        let initial = IndexRootOptions::from_config(&config.indexing)?;
        let scan = initial.scan.clone();
        let options: SharedOptions = Arc::new(RwLock::new(Arc::new(initial)));
        let mut write_conn = db::open(db_path)?;
        files::reset_indexing_to_pending(&write_conn)?;
        requeue_on_model_change(
            &mut write_conn,
            &ModelIds {
                text: components.text.as_deref().map(|e| e.model_id()),
                image: components.image.as_deref().map(|e| e.model_id()),
                ocr: components.ocr.as_deref().map(|o| o.engine_id()),
            },
        )?;
        requeue_missing(&mut write_conn, &components.running())?;
        let scheduler_conn = db::open(db_path)?;
        let pause = Arc::new(Pause::default());
        pause.user.store(
            meta::get(&write_conn, "paused")?.as_deref() == Some("1"),
            Ordering::SeqCst,
        );

        let ctx = Arc::new(IndexContext {
            embedder: components.text,
            ocr: components.ocr,
            image_embedder: components.image,
            ..IndexContext::default()
        });
        let (total_memory, workers) = resources::machine(config.indexing.worker_threads);
        let max_in_flight = workers * 2 + 2;

        let (write_tx, write_rx) = crossbeam_channel::unbounded::<WriteJob>();
        let (done_tx, done_rx) = crossbeam_channel::unbounded::<i64>();
        let (wake_tx, wake_rx) = crossbeam_channel::bounded::<()>(1);
        let (extract_tx, extract_rx) = crossbeam_channel::bounded::<Job>(max_in_flight);
        // Room for everything in flight (the scheduler already caps that), so
        // the embed worker finds small files queued together to batch.
        let (embed_tx, embed_rx) = crossbeam_channel::bounded::<(Job, Fresh)>(max_in_flight);
        let stats = Arc::new(Stats::default());
        let stop = Arc::new(AtomicBool::new(false));
        let search_pending = SearchPending::default();
        let force_low_memory = Arc::new(AtomicBool::new(false));
        let (monitor_wake, monitor_rx) = crossbeam_channel::bounded::<()>(1);

        // Step 3: watch before scanning. Events that arrive during the scan
        // queue behind it on the writer's channel and are applied afterwards.
        let enabled: Vec<_> = roots::list(&write_conn)?
            .into_iter()
            .filter(|r| r.enabled)
            .collect();
        let accessible: Vec<_> = enabled
            .iter()
            .filter(|r| FsProbe.probe(&r.path) == RootAccess::Ok)
            .cloned()
            .collect();
        let (watchers, failed) = watcher::start(&accessible, &write_tx);
        for root in &accessible {
            let failed_now = failed.contains(&root.id);
            if failed_now {
                roots::set_status(&write_conn, root.id, Health::WatchFailed)?;
            } else if root.status == Health::WatchFailed {
                roots::set_status(&write_conn, root.id, Health::Ok)?;
            }
        }
        // Roots polled instead of watched: failed ones, and any not accessible.
        let unwatched = Arc::new(AtomicBool::new(
            accessible.len() < enabled.len() || !failed.is_empty(),
        ));
        // Never sent on: dropping it at shutdown stops the ticker and status threads.
        let (closer, closed) = crossbeam_channel::bounded::<()>(0);
        let status = StatusSource {
            scan: scan.clone(),
            reader: Arc::new(Mutex::new(db::open(db_path)?)),
            stats: stats.clone(),
            pause: pause.clone(),
        };
        let subscribers: Subscribers = Arc::default();

        let inner = Arc::new(Inner {
            stop: stop.clone(),
            scan,
            options: options.clone(),
            watchers: Mutex::new(Some(watchers)),
            unwatched: unwatched.clone(),
            closer: Mutex::new(Some(closer)),
            threads: Mutex::new(Vec::new()),
            write_tx: write_tx.clone(),
            status: status.clone(),
            subscribers: subscribers.clone(),
            search_pending: search_pending.clone(),
            force_low_memory: force_low_memory.clone(),
            monitor_wake,
        });
        let handle = EngineHandle { inner };

        handle.spawn("magi-writer", {
            let (options, stats, wake) = (options.clone(), stats.clone(), wake_tx.clone());
            move || writer::run(write_conn, options, write_rx, done_tx, wake, stats)
        })?;

        // Step 4: the reconciliation scan. `start` does not wait for it (status
        // and keyword search work meanwhile); the scheduler does, so nothing is
        // extracted while a moved file is still waiting to be matched.
        let scan = handle.rescan();

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
            let stats = stats.clone();
            handle.spawn(&format!("magi-extract-{n}"), move || {
                extract_worker(&ctx, &options, &stats, &stop, rx, &embed_tx, &write_tx)
            })?;
        }
        drop((extract_rx, embed_tx));

        // Step 6: the queue, newest first, is the scheduler's job from here.
        handle.spawn("magi-scheduler", {
            let (write_tx, stop, pause) = (write_tx.clone(), stop.clone(), pause.clone());
            move || {
                // A failed scan is logged; indexing carries on with whatever
                // is pending.
                loop {
                    match scan.recv_timeout(POLL_INTERVAL) {
                        Err(RecvTimeoutError::Timeout) if !stop.load(Ordering::SeqCst) => {}
                        Ok(Err(error)) => {
                            tracing::error!(%error, "the startup scan failed");
                            break;
                        }
                        _ => break,
                    }
                }
                scheduler_thread(
                    &scheduler_conn,
                    Scheduler::new(max_in_flight),
                    &stop,
                    &pause,
                    (&done_rx, &wake_rx),
                    &write_tx,
                    &extract_tx,
                )
            }
        })?;
        handle.spawn("magi-ticker", {
            let (hours, closed) = (config.indexing.reconcile_interval_hours, closed.clone());
            move || poller::run(closed, write_tx, hours, unwatched)
        })?;
        handle.spawn("magi-status", move || {
            status_thread(&status, &subscribers, &closed)
        })?;
        let monitor = resources::Monitor {
            ctx,
            pause,
            force_low_memory,
            pause_on_battery: config.indexing.pause_on_battery,
            idle: resources::idle_unload(config.models.idle_unload_minutes, total_memory),
        };
        handle.spawn("magi-monitor", move || {
            resources::run(monitor, &stop, monitor_rx)
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
    /// Ends a running scan early (quit, or a folder switched off).
    scan: Arc<ScanControl>,
    options: SharedOptions,
    watchers: Mutex<Option<Watchers>>,
    /// Set while some root is polled instead of watched.
    unwatched: Arc<AtomicBool>,
    /// Dropped at shutdown, which stops the ticker and status threads.
    closer: Mutex<Option<Sender<()>>>,
    /// Spawn order: writer, embed, extract workers, scheduler, ticker, status,
    /// monitor.
    threads: Mutex<Vec<JoinHandle<()>>>,
    write_tx: Sender<WriteJob>,
    status: StatusSource,
    subscribers: Subscribers,
    search_pending: SearchPending,
    force_low_memory: Arc<AtomicBool>,
    monitor_wake: Sender<()>,
}

type Subscribers = Arc<Mutex<Vec<Sender<IndexStatus>>>>;

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
        self.inner.status.stats.snapshot()
    }

    /// The index at a glance (SPEC.md §5.7 `get_status`).
    pub fn status(&self) -> Result<IndexStatus> {
        self.inner.status.read()
    }

    /// Every status change from now on (SPEC.md §5.3 engine events: state,
    /// progress, root status, permission problems as root status). Checked
    /// twice a second; dropping the receiver unsubscribes.
    pub fn subscribe(&self) -> Receiver<IndexStatus> {
        let (tx, rx) = crossbeam_channel::unbounded();
        lock(&self.inner.subscribers).push(tx);
        rx
    }

    /// Stops starting new files until [`resume`](Self::resume), across
    /// restarts too (SPEC.md §6.4). Files already in the pipeline finish, and
    /// changes are still queued.
    pub fn pause(&self) -> Result<()> {
        self.set_user_pause(true)
    }

    pub fn resume(&self) -> Result<()> {
        self.set_user_pause(false)
    }

    fn set_user_pause(&self, on: bool) -> Result<()> {
        self.inner.status.pause.user.store(on, Ordering::SeqCst);
        self.write(move |conn| meta::set(conn, "paused", if on { "1" } else { "0" }))
    }

    /// Queues every file in `error` again (SPEC.md §5.7 `retry_errors`).
    /// Returns how many.
    pub fn retry_errors(&self) -> Result<usize> {
        self.write(|conn| files::retry_errors(conn))
    }

    /// Runs `write` on the writer thread, in order with everything queued
    /// before it, and waits for its result.
    fn write<T: Send + 'static>(
        &self,
        write: impl FnOnce(&mut Connection) -> Result<T> + Send + 'static,
    ) -> Result<T> {
        let stopped = || Error::Engine("the engine is stopped".into());
        let (reply, result) = crossbeam_channel::bounded(1);
        self.inner
            .write_tx
            .send(WriteJob::Exec(Box::new(move |conn| {
                let _ = reply.send(write(conn));
            })))
            .map_err(|_| stopped())?;
        result.recv().map_err(|_| stopped())?
    }

    /// Walks every enabled root again: new and changed files become `pending`,
    /// vanished ones are deleted, moves are matched. The reply arrives when the
    /// scan is done; indexing then proceeds on its own. The watcher
    /// uses this for overflow and periodic safety nets.
    pub fn rescan(&self) -> Receiver<Result<ScanSummary>> {
        let (reply, result) = crossbeam_channel::bounded(1);
        // A stopped engine drops `reply`, which the caller sees as a
        // disconnected receiver.
        let _ = self.inner.write_tx.send(WriteJob::Reconcile(Some(reply)));
        result
    }

    /// Swaps in new indexing options (exclusions, hidden files, size limits) and
    /// rescans, so files that are newly excluded are purged and files that are
    /// newly allowed are indexed.
    pub fn apply_indexing_config(
        &self,
        config: &IndexingConfig,
    ) -> Result<Receiver<Result<ScanSummary>>> {
        let mut options = IndexRootOptions::from_config(config)?;
        options.scan = self.inner.scan.clone();
        let options = Arc::new(options);
        *self
            .inner
            .options
            .write()
            .unwrap_or_else(PoisonError::into_inner) = options;
        Ok(self.rescan())
    }

    /// Indexing yields to searches that hold a [`SearchGuard`] (SPEC.md §5.3,
    /// the priority lock). M6's search command takes one per query.
    pub fn search_pending(&self) -> SearchPending {
        self.inner.search_pending.clone()
    }

    /// Whether indexing is paused, by the user or for low memory or battery.
    /// Files already in the pipeline finish; no new ones start.
    pub fn is_paused(&self) -> bool {
        self.inner.status.pause.any()
    }

    /// Test hook (SPEC.md §7 M5 item 17): acts as if available memory were
    /// below 1 GB until called with `false`.
    #[doc(hidden)]
    pub fn simulate_low_memory(&self, low: bool) {
        self.inner.force_low_memory.store(low, Ordering::SeqCst);
        let _ = self.inner.monitor_wake.try_send(());
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
        self.scan.stop();
        // No new events, and the ticker and status threads stop waiting.
        drop(lock(&self.watchers).take());
        drop(lock(&self.closer).take());
        let _ = self.monitor_wake.try_send(());
        let mut threads = std::mem::take(&mut *lock(&self.threads));
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

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}
