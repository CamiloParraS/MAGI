//! Root management on the engine: add, remove, switch on and off, watch.

use std::path::Path;
use std::sync::atomic::Ordering;

use crate::db::files;
use crate::db::roots::{self, Health, Root};
use crate::dto::RootStatus;
use crate::error::Result;
use crate::index::writer::WriteJob;
use crate::platform::{FsProbe, PermissionProbe, RootAccess};
use crate::watch::watcher;

use super::{EngineHandle, lock};

impl EngineHandle {
    /// Registers a root (SPEC.md §5.7 `add_root`), rejecting a missing,
    /// duplicate or already-covered path and collapsing roots inside it (their
    /// indexed files are kept), then probes, watches and scans it. The status
    /// returned is the probe's result.
    pub fn add_root(&self, path: &Path) -> Result<RootStatus> {
        self.begin_scan();
        let added = self.add_root_inner(path);
        if added.is_err() {
            self.inner.status.stats.end_scan();
        }
        added
    }

    /// The status reads `Scanning` from the moment a folder is added: watching
    /// a big one takes about as long as walking it, with nothing else to show.
    fn begin_scan(&self) {
        self.inner.scan.reset_seen();
        self.inner.status.stats.begin_scan();
    }

    fn add_root_inner(&self, path: &Path) -> Result<RootStatus> {
        let path = path.to_path_buf();
        // Collapsed roots' indexed files are now this root's.
        let (root, collapsed, counts) = self.write(move |conn| {
            let (root, collapsed) = roots::add_collapsing(conn, &path)?;
            roots::set_access(conn, root.id, &FsProbe.probe(&root.path))?;
            let counts = files::count_indexed_by_root(conn)?;
            Ok((roots::get(conn, root.id)?, collapsed, counts))
        })?;
        for id in collapsed {
            self.unwatch(id);
        }
        self.scan_then_watch(&root);
        Ok(RootStatus::new(root, &counts))
    }

    /// Scans a root at once and watches it meanwhile: watching a big folder
    /// takes as long as walking it, and the first files should not wait for
    /// it. The second scan, once the watcher runs, catches what changed in
    /// between.
    /// ponytail: a root removed or switched off during the watch keeps a
    /// watcher whose events the writer ignores; drop it then if that shows up.
    fn scan_then_watch(&self, root: &Root) {
        let _ = self.inner.write_tx.send(WriteJob::ReconcileRoot(root.id));
        let (engine, root) = (self.clone(), root.clone());
        std::thread::spawn(move || {
            if let Err(e) = engine.watch(root.clone()) {
                tracing::warn!(root = %root.path.display(), error = %e, "could not watch root");
            }
            let _ = engine.inner.write_tx.send(WriteJob::ReconcileRoot(root.id));
        });
    }

    /// Stops watching a root and purges everything indexed under it (item 12).
    pub fn remove_root(&self, id: i64) -> Result<()> {
        self.inner.scan.switch_off(id);
        self.unwatch(id);
        self.write(move |conn| roots::remove(conn, id))
    }

    /// A disabled root keeps its rows but is not watched, indexed or searched;
    /// enabling it watches and scans it again.
    pub fn set_root_enabled(&self, id: i64, enabled: bool) -> Result<()> {
        // Before the write, which waits behind any scan the writer is running.
        if !enabled {
            self.inner.scan.switch_off(id);
        }
        let root = self.write(move |conn| {
            roots::set_enabled(conn, id, enabled)?;
            roots::get(conn, id)
        })?;
        if enabled {
            self.inner.scan.switch_on(id);
            self.begin_scan();
            self.scan_then_watch(&root);
        } else {
            self.unwatch(id);
        }
        Ok(())
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
}
