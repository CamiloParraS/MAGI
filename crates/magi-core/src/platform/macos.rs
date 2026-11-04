//! macOS implementations of the `platform` traits (see SPEC.md §6.1).

/// Matches `xtask`'s `vendor/pdfium/<dir>/` layout (see `xtask/src/main.rs`).
#[cfg(target_arch = "aarch64")]
pub const PDFIUM_VENDOR_DIR: &str = "mac-arm64";
#[cfg(target_arch = "x86_64")]
pub const PDFIUM_VENDOR_DIR: &str = "mac-x64";
pub const PDFIUM_LIBRARY_FILENAME: &str = "libpdfium.dylib";
/// See `platform::windows::PDFIUM_LIBRARY_SUBDIR` for why this differs
/// from Windows.
pub const PDFIUM_LIBRARY_SUBDIR: &str = "lib";

/// Matches `xtask fetch-onnxruntime`'s `vendor/onnxruntime/<dir>/` layout.
/// Note: Microsoft's v1.28.0 release has no Intel-Mac (`mac-x64`) build, so
/// that target has no vendored binary yet even though the string exists.
#[cfg(target_arch = "aarch64")]
pub const ONNXRUNTIME_VENDOR_DIR: &str = "mac-arm64";
#[cfg(target_arch = "x86_64")]
pub const ONNXRUNTIME_VENDOR_DIR: &str = "mac-x64";
pub const ONNXRUNTIME_LIBRARY_FILENAME: &str = "libonnxruntime.dylib";

use std::os::macos::fs::MetadataExt;

/// `st_flags` come from `stat`: reading them never downloads the file.
pub fn is_cloud_only(metadata: &std::fs::Metadata) -> bool {
    super::macos_flags_dataless(metadata.st_flags())
}

/// ponytail: shells out to `pmset -g batt`; IOKit's power-source API if the
/// once-a-minute process spawn ever shows up in idle CPU.
pub fn on_battery() -> Option<bool> {
    let output = std::process::Command::new("pmset")
        .args(["-g", "batt"])
        .output()
        .ok()?;
    super::pmset_on_battery(&String::from_utf8_lossy(&output.stdout))
}

/// The filesystem `path` is on, by `statfs`'s type name (`smbfs`, `apfs`, ...).
/// A path that cannot be stat'ed is not a network drive: the probe reports it.
pub fn is_network_drive(path: &std::path::Path) -> bool {
    use std::os::unix::ffi::OsStrExt;
    let Ok(path) = std::ffi::CString::new(path.as_os_str().as_bytes()) else {
        return false;
    };
    // SAFETY: `path` is NUL-terminated and outlives the call; `stat` is a
    // plain C struct that `statfs` fills on success, and its type name is
    // NUL-terminated.
    unsafe {
        let mut stat: libc::statfs = std::mem::zeroed();
        if libc::statfs(path.as_ptr(), &mut stat) != 0 {
            return false;
        }
        let name = std::ffi::CStr::from_ptr(stat.f_fstypename.as_ptr());
        super::is_network_fs_type(&name.to_string_lossy())
    }
}

/// `Accessory`: no Dock icon and no app menu bar, windows still open and take
/// focus (`cx.activate`). Must run on the main thread, after GPUI's own
/// `Regular` (it sets that in `applicationDidFinishLaunching`, which is where
/// the callback given to `Application::run` is called from).
pub fn hide_dock_icon() {
    use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy};
    let Some(main) = objc2::MainThreadMarker::new() else {
        tracing::warn!("not on the main thread; the Dock icon stays");
        return;
    };
    let accepted = NSApplication::sharedApplication(main)
        .setActivationPolicy(NSApplicationActivationPolicy::Accessory);
    if !accepted {
        tracing::warn!("macOS refused the Accessory activation policy; the Dock icon stays");
    }
}

pub fn lower_current_thread() {
    // SAFETY: only changes the calling thread's QoS class.
    let rc =
        unsafe { libc::pthread_set_qos_class_self_np(libc::qos_class_t::QOS_CLASS_UTILITY, 0) };
    if rc != 0 {
        tracing::debug!(rc, "could not lower thread QoS");
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
struct CGPoint {
    x: f64,
    y: f64,
}

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGEventCreate(source: *const std::ffi::c_void) -> *mut std::ffi::c_void;
    fn CGEventGetLocation(event: *const std::ffi::c_void) -> CGPoint;
    fn CGGetDisplaysWithPoint(
        point: CGPoint,
        max_displays: u32,
        displays: *mut u32,
        matching_display_count: *mut u32,
    ) -> i32;
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFRelease(cf: *const std::ffi::c_void);
}

/// A null-source event carries the current cursor location, in the global
/// display coordinates `CGGetDisplaysWithPoint` takes.
pub fn display_under_cursor() -> Option<u64> {
    // SAFETY: the event is checked for null and released once; the out
    // pointers are valid for one display id and one count.
    unsafe {
        let event = CGEventCreate(std::ptr::null());
        if event.is_null() {
            return None;
        }
        let point = CGEventGetLocation(event);
        CFRelease(event);
        let (mut display, mut count) = (0u32, 0u32);
        (CGGetDisplaysWithPoint(point, 1, &mut display, &mut count) == 0 && count == 1)
            .then_some(u64::from(display))
    }
}

/// Puts one file and its path text on the pasteboard through AppleScript: a
/// record sets `furl` (Finder pastes the file) and `utf8` text together.
// ponytail: one file only (a list of furl is awkward in AppleScript); NSPasteboard
// writeObjects would take several.
pub fn copy_files(paths: &[std::path::PathBuf], text: &str) -> bool {
    let [path] = paths else { return false };
    let quote = |s: &str| s.replace('\\', "\\\\").replace('"', "\\\"");
    let script = format!(
        "set the clipboard to {{«class furl»:(POSIX file \"{}\"), string:\"{}\"}}",
        quote(&path.to_string_lossy()),
        quote(text)
    );
    std::process::Command::new("osascript")
        .args(["-e", &script])
        .status()
        .is_ok_and(|s| s.success())
}

#[cfg(test)]
mod copy_tests {
    /// Touches the real pasteboard, so it only runs on request (the
    /// `clipboard` workflow): `cargo test -p magi-core copied_files -- --ignored`.
    #[test]
    #[ignore = "overwrites the clipboard"]
    fn copied_files_reach_the_clipboard() {
        let file = std::env::temp_dir().join("magi-copy-as-file.txt");
        std::fs::write(&file, "x").unwrap();
        assert!(super::copy_files(
            std::slice::from_ref(&file),
            &file.to_string_lossy()
        ));
    }
}
