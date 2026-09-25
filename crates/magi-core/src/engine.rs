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

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError, RwLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, RecvTimeoutError, Sender, select};
use rusqlite::Connection;

use crate::config::{Config, IndexingConfig};
use crate::db::files::FileState;
use crate::db::roots::{Health, Root};
use crate::db::{self, files, meta, roots};
use crate::dto::{IndexState, IndexStatus, RootStatus};
use crate::embed::{ImageEmbedder, TextEmbedder};
use crate::error::{Error, Result};
use crate::index::lifecycle::{self, FileStep};
use crate::index::pipeline::{
    EMBED_BATCH, Extracted, Fresh, IndexContext, IndexRootOptions, Job, embed_group, prepare,
};
use crate::index::resources::{self, Pause};
use crate::index::scheduler::{Action, Now, Scheduler};
use crate::index::writer::{self, SharedOptions, Stats, StatsSnapshot, WriteJob};
use crate::index::{ModelIds, requeue_on_model_change};
use crate::ocr::OcrEngine;
use crate::platform::{FsProbe, Os, PermissionProbe, RootAccess, ThreadPriority};
use crate::watch::reconcile::ScanSummary;
use crate::watch::watcher::Watchers;
use crate::watch::{poller, watcher};

/// How often the scheduler looks at the queue when nothing wakes it sooner.
///
/// ponytail: fixed polling; back off when idle if idle CPU shows up in the
/// Slice 8 benchmark.
const POLL_INTERVAL: Duration = Duration::from_millis(250);
/// The longest an embed batch yields to a waiting search, so a leaked
/// [`SearchGuard`] cannot stall indexing for good.
const MAX_YIELD: Duration = Duration::from_secs(5);
/// How often the status thread checks whether anything changed.
const STATUS_INTERVAL: Duration = Duration::from_millis(500);

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
        let options: SharedOptions = Arc::new(RwLock::new(Arc::new(
            IndexRootOptions::from_config(&config.indexing)?,
        )));
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
        let pause = Arc::new(Pause::default());
        pause.user.store(
            meta::get(&write_conn, "paused")?.as_deref() == Some("1"),
            Ordering::SeqCst,
        );

        let ctx = Arc::new(IndexContext {
            image_embedder: image,
            ocr,
            ..IndexContext::new(text)
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
            reader: Arc::new(Mutex::new(db::open(db_path)?)),
            stats: stats.clone(),
            pause: pause.clone(),
        };
        let subscribers: Subscribers = Arc::default();

        let inner = Arc::new(Inner {
            stop: stop.clone(),
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

    /// Registers a root (SPEC.md §5.7 `add_root`), rejecting a missing,
    /// duplicate or already-covered path and collapsing roots inside it (their
    /// indexed files are kept), then probes, watches and scans it. The status
    /// returned is the probe's result.
    pub fn add_root(&self, path: &Path) -> Result<RootStatus> {
        let path = path.to_path_buf();
        let (root, collapsed) = self.write(move |conn| {
            let (root, collapsed) = roots::add_collapsing(conn, &path)?;
            roots::set_access(conn, root.id, &FsProbe.probe(&root.path))?;
            Ok((roots::get(conn, root.id)?, collapsed))
        })?;
        for id in collapsed {
            self.unwatch(id);
        }
        let root = self.watch(root)?;
        let _ = self.inner.write_tx.send(WriteJob::ReconcileRoot(root.id));
        Ok(root.into())
    }

    /// Stops watching a root and purges everything indexed under it (item 12).
    pub fn remove_root(&self, id: i64) -> Result<()> {
        self.unwatch(id);
        self.write(move |conn| roots::remove(conn, id))
    }

    /// A disabled root keeps its rows but is not watched, indexed or searched;
    /// enabling it watches and scans it again.
    pub fn set_root_enabled(&self, id: i64, enabled: bool) -> Result<()> {
        let root = self.write(move |conn| {
            roots::set_enabled(conn, id, enabled)?;
            roots::get(conn, id)
        })?;
        if enabled {
            let id = self.watch(root)?.id;
            let _ = self.inner.write_tx.send(WriteJob::ReconcileRoot(id));
        } else {
            self.unwatch(id);
        }
        Ok(())
    }

    /// Queues every file in `error` again (SPEC.md §5.7 `retry_errors`).
    /// Returns how many.
    pub fn retry_errors(&self) -> Result<usize> {
        self.write(|conn| files::retry_errors(conn))
    }

    /// Starts watching `root` if it is readable. One that cannot be watched is
    /// marked `watch_failed` and polled; one watched again is `ok` again.
    fn watch(&self, root: Root) -> Result<Root> {
        if FsProbe.probe(&root.path) != RootAccess::Ok {
            self.inner.unwatched.store(true, Ordering::SeqCst);
            return Ok(root);
        }
        let (started, failed) = watcher::start(std::slice::from_ref(&root), &self.inner.write_tx);
        if let Some(watchers) = lock(&self.inner.watchers).as_mut() {
            watchers.extend(started);
        }
        let status = if failed.is_empty() {
            Health::Ok
        } else {
            self.inner.unwatched.store(true, Ordering::SeqCst);
            Health::WatchFailed
        };
        if root.status.readable() && root.status != status {
            let id = root.id;
            return self.write(move |conn| {
                roots::set_status(conn, id, status)?;
                roots::get(conn, id)
            });
        }
        Ok(root)
    }

    fn unwatch(&self, id: i64) {
        if let Some(watchers) = lock(&self.inner.watchers).as_mut() {
            watchers.remove(&id);
        }
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
    /// scan is done; indexing then proceeds on its own. The watcher (M5 Slice 4)
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
        let options = Arc::new(IndexRootOptions::from_config(config)?);
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

/// What [`IndexStatus`] is read from, shared by the handle and the status
/// thread. The reader connection only reads (WAL), so it never waits on the
/// writer.
#[derive(Clone)]
struct StatusSource {
    reader: Arc<Mutex<Connection>>,
    stats: Arc<Stats>,
    pause: Arc<Pause>,
}

impl StatusSource {
    fn read(&self) -> Result<IndexStatus> {
        let conn = lock(&self.reader);
        let counts = files::count_states(&conn)?;
        let count = |state: FileState| counts.get(&state).copied().unwrap_or(0);
        let queued = count(FileState::Pending) + count(FileState::Indexing);
        let state = if self.pause.any() {
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
            queued,
            indexed: count(FileState::Indexed),
            skipped: count(FileState::Skipped),
            errors: count(FileState::Error),
            current_file: current_file.map(|p| p.to_string_lossy().into_owned()),
            roots: roots::list(&conn)?
                .into_iter()
                .map(RootStatus::from)
                .collect(),
        })
    }
}

/// Sends the status to every subscriber whenever it changes. The database is
/// read only after the writer applied something, or the pause or scan state
/// flipped, so an idle engine costs a few atomic loads twice a second.
fn status_thread(source: &StatusSource, subscribers: &Subscribers, closed: &Receiver<()>) {
    let mut seen = None;
    let mut last = None;
    while let Err(RecvTimeoutError::Timeout) = closed.recv_timeout(STATUS_INTERVAL) {
        let now = (
            source.stats.generation(),
            source.pause.any(),
            source.stats.is_scanning(),
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
    let roots: std::collections::HashMap<i64, PathBuf> = match roots::list(conn) {
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

fn extract_worker(
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
            Ok(Extracted::Retry { message, locked }) => FileStep::Retry {
                file_id: job.stored.id,
                message,
                locked,
            },
            Err(_) => FileStep::retry(job.stored.id, "extraction panicked".into()),
        };
        let _ = write_tx.send(next.into());
    }
}

fn embed_worker(
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
                let _ = write_tx.send(FileStep::retry(id, "embedding panicked".into()).into());
            }
            continue;
        };
        for (job, embedded) in results {
            let next = match embedded {
                Ok(embedded) => FileStep::Store {
                    job: Box::new(job),
                    embedded: Box::new(embedded),
                },
                Err(e) => FileStep::retry(job.stored.id, e.to_string()),
            };
            let _ = write_tx.send(next.into());
        }
    }
}
