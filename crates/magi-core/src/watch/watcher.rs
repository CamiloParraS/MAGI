//! One `notify` watcher per root, debounced (~2 s), reporting *which paths
//! changed* to the writer as [`WriteJob::Paths`] (SPEC.md §5.4 runtime events).
//! What each change means (new, modified, removed, renamed, moved out of the
//! roots, now excluded) is decided by [`scan_paths`], which looks at the
//! disk instead of trusting the event kind: the event kinds differ by OS, and
//! renames arrive as pairs, halves, or delete plus create.
//!
//! [`scan_paths`]: super::reconcile::scan_paths

use std::collections::HashSet;
use std::path::PathBuf;
use std::time::Duration;

use crossbeam_channel::Sender;
use notify::{RecommendedWatcher, RecursiveMode};
use notify_debouncer_full::{DebounceEventResult, Debouncer, RecommendedCache, new_debouncer};

use crate::db::roots::Root;
use crate::index::writer::WriteJob;

/// How long events are held so a burst (a save, a rename pair, a copy) arrives
/// as one batch.
const DEBOUNCE: Duration = Duration::from_secs(2);

/// The running watchers. Dropping this stops them.
pub type Watchers = Vec<Debouncer<RecommendedWatcher, RecommendedCache>>;

/// Starts a watcher on each root. Returns the ones that could not be watched
/// (inotify limit, a network drive, a vanished folder) so the caller can mark
/// them `watch_failed` and leave them to the polling reconciliation.
///
/// Called before the first scan: events that arrive during it queue behind the
/// scan on the writer's channel and are applied afterwards.
pub(crate) fn start(roots: &[Root], jobs: &Sender<WriteJob>) -> (Watchers, Vec<i64>) {
    let mut debouncers = Vec::new();
    let mut failed = Vec::new();
    for root in roots {
        if crate::platform::is_network_drive(&root.path) {
            tracing::info!(root = %root.path.display(), "network drive; polling instead of watching");
            failed.push(root.id);
            continue;
        }
        match watch_root(root, jobs.clone()) {
            Ok(debouncer) => debouncers.push(debouncer),
            Err(e) => {
                if matches!(e.kind, notify::ErrorKind::MaxFilesWatch) {
                    tracing::warn!(
                        root = %root.path.display(),
                        "inotify watch limit reached; polling instead. Raise it with \
                         `sudo sysctl fs.inotify.max_user_watches=524288` and persist it \
                         in /etc/sysctl.d/"
                    );
                } else {
                    tracing::warn!(root = %root.path.display(), error = %e, "cannot watch root; polling instead");
                }
                failed.push(root.id);
            }
        }
    }
    (debouncers, failed)
}

fn watch_root(
    root: &Root,
    jobs: Sender<WriteJob>,
) -> notify::Result<Debouncer<RecommendedWatcher, RecommendedCache>> {
    let mut debouncer = new_debouncer(DEBOUNCE, None, move |result: DebounceEventResult| {
        if let Some(job) = job_for(result) {
            let _ = jobs.send(job);
        }
    })?;
    debouncer.watch(&root.path, RecursiveMode::Recursive)?;
    Ok(debouncer)
}

/// The writer job for one debounced batch. An error, or an event that says the
/// OS dropped events (overflow), means the paths are not to be trusted: rescan.
fn job_for(result: DebounceEventResult) -> Option<WriteJob> {
    match result {
        Err(errors) => {
            tracing::warn!(?errors, "watcher error; rescanning");
            Some(WriteJob::Reconcile(None))
        }
        Ok(events) => {
            if events.iter().any(|e| e.need_rescan()) {
                tracing::warn!("watcher overflow; rescanning");
                return Some(WriteJob::Reconcile(None));
            }
            // Reads and opens are not changes.
            let paths: HashSet<PathBuf> = events
                .iter()
                .filter(|e| !e.kind.is_access())
                .flat_map(|e| e.paths.iter().cloned())
                .collect();
            (!paths.is_empty()).then(|| WriteJob::Paths(paths.into_iter().collect()))
        }
    }
}
