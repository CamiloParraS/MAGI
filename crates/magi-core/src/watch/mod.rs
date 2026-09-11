//! File-system watching: per-root watcher, reconciliation scans, and the
//! polling fallback. Implemented in M5+ (see SPEC.md §5.4).

pub mod poller;
pub mod reconcile;
pub mod watcher;
