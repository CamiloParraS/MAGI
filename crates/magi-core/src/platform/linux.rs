//! Linux implementations of the `platform` traits (see SPEC.md §6.3).

/// Matches `xtask`'s `vendor/pdfium/<dir>/` layout (see `xtask/src/main.rs`).
pub const PDFIUM_VENDOR_DIR: &str = "linux-x64";
pub const PDFIUM_LIBRARY_FILENAME: &str = "libpdfium.so";
/// See `platform::windows::PDFIUM_LIBRARY_SUBDIR` for why this differs
/// from Windows.
pub const PDFIUM_LIBRARY_SUBDIR: &str = "lib";

/// Matches `xtask fetch-onnxruntime`'s `vendor/onnxruntime/<dir>/` layout.
pub const ONNXRUNTIME_VENDOR_DIR: &str = "linux-x64";
pub const ONNXRUNTIME_LIBRARY_FILENAME: &str = "libonnxruntime.so";

/// Reads the kernel's `max_user_watches` inotify limit.
pub fn inotify_watch_limit() -> Option<u64> {
    std::fs::read_to_string("/proc/sys/fs/inotify/max_user_watches")
        .ok()
        .and_then(|s| s.trim().parse().ok())
}

/// Reads `/sys/class/power_supply/*/{type,online}`.
pub fn on_battery() -> Option<bool> {
    let supplies: Vec<_> = std::fs::read_dir("/sys/class/power_supply")
        .ok()?
        .flatten()
        .filter_map(|entry| {
            let read = |name| std::fs::read_to_string(entry.path().join(name)).ok();
            Some(super::PowerSupply {
                kind: read("type")?.trim().to_string(),
                online: read("online").map(|s| s.trim() == "1"),
            })
        })
        .collect();
    super::power_supplies_on_battery(&supplies)
}

/// `nice(10)` for the calling thread: on Linux the nice value is per thread,
/// and `who = 0` means the caller.
pub fn lower_current_thread() {
    // SAFETY: plain syscall wrapper with no pointers.
    if unsafe { libc::setpriority(libc::PRIO_PROCESS, 0, 10) } != 0 {
        tracing::debug!(error = %std::io::Error::last_os_error(), "could not lower thread priority");
    }
}
