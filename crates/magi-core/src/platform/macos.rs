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
