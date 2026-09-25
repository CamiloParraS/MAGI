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

pub fn lower_current_thread() {
    // SAFETY: only changes the calling thread's QoS class.
    let rc =
        unsafe { libc::pthread_set_qos_class_self_np(libc::qos_class_t::QOS_CLASS_UTILITY, 0) };
    if rc != 0 {
        tracing::debug!(rc, "could not lower thread QoS");
    }
}
