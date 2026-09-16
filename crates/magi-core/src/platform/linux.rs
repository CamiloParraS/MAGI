//! Linux implementations of the `platform` traits (see SPEC.md §6.3).

/// Matches `xtask`'s `vendor/pdfium/<dir>/` layout (see `xtask/src/main.rs`).
pub const PDFIUM_VENDOR_DIR: &str = "linux-x64";
pub const PDFIUM_LIBRARY_FILENAME: &str = "libpdfium.so";
/// See `platform::windows::PDFIUM_LIBRARY_SUBDIR` for why this differs
/// from Windows.
pub const PDFIUM_LIBRARY_SUBDIR: &str = "lib";

/// Reads the kernel's `max_user_watches` inotify limit.
pub fn inotify_watch_limit() -> Option<u64> {
    std::fs::read_to_string("/proc/sys/fs/inotify/max_user_watches")
        .ok()
        .and_then(|s| s.trim().parse().ok())
}
