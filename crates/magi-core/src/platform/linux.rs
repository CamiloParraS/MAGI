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

/// `text/uri-list` body for `paths`: one `file://` URI per CRLF-terminated
/// line, bytes outside the unreserved set and `/` percent-encoded. Pure, so
/// it is tested on every OS.
pub fn uri_list(paths: &[std::path::PathBuf]) -> String {
    let mut out = String::new();
    for path in paths {
        out.push_str("file://");
        for b in path.to_string_lossy().bytes() {
            if b.is_ascii_alphanumeric() || b"-._~/".contains(&b) {
                out.push(b as char);
            } else {
                out.push_str(&format!("%{b:02X}"));
            }
        }
        out.push_str("\r\n");
    }
    out
}

/// Puts `paths` on the clipboard as `text/uri-list` (`wl-copy` on Wayland,
/// `xclip` otherwise). The tools own one target, so the path text is not set
/// with it: that is what `ui.copy_as_file = false` is for. Whether it took.
pub fn copy_files(paths: &[std::path::PathBuf]) -> bool {
    use std::io::Write;
    use std::process::{Command, Stdio};
    let mut cmd = if std::env::var_os("WAYLAND_DISPLAY").is_some() {
        let mut c = Command::new("wl-copy");
        c.args(["--type", "text/uri-list"]);
        c
    } else {
        let mut c = Command::new("xclip");
        c.args(["-selection", "clipboard", "-t", "text/uri-list"]);
        c
    };
    // The tool forks to keep owning the selection; null output so it cannot
    // hold our pipes open.
    let Ok(mut child) = cmd
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    else {
        return false;
    };
    let wrote = child
        .stdin
        .take()
        .is_some_and(|mut stdin| stdin.write_all(uri_list(paths).as_bytes()).is_ok());
    wrote && child.wait().is_ok_and(|s| s.success())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uris_are_percent_encoded() {
        let list = uri_list(&["/home/a b/é.txt".into()]);
        assert_eq!(list, "file:///home/a%20b/%C3%A9.txt\r\n");
    }

    /// Touches the real clipboard (needs `xclip` and a display), so it only
    /// runs on request (the `clipboard` workflow).
    #[test]
    #[ignore = "overwrites the clipboard"]
    fn copied_files_reach_the_clipboard() {
        let file = std::env::temp_dir().join("magi-copy-as-file.txt");
        std::fs::write(&file, "x").unwrap();
        assert!(copy_files(std::slice::from_ref(&file)));
    }
}
