//! Linux implementations of the `platform` traits (see SPEC.md §6.3).

/// Reads the kernel's `max_user_watches` inotify limit.
pub fn inotify_watch_limit() -> Option<u64> {
    std::fs::read_to_string("/proc/sys/fs/inotify/max_user_watches")
        .ok()
        .and_then(|s| s.trim().parse().ok())
}
