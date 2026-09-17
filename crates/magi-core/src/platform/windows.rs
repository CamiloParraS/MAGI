//! Windows implementations of the `platform` traits (see SPEC.md §6.2).

/// Matches `xtask`'s `vendor/pdfium/<dir>/` layout (see `xtask/src/main.rs`).
pub const PDFIUM_VENDOR_DIR: &str = "win-x64";
pub const PDFIUM_LIBRARY_FILENAME: &str = "pdfium.dll";
/// pdfium-binaries puts the Windows DLL under `bin/` (the import library
/// goes in `lib/`, unused here); Linux/macOS have no `bin/` and put the
/// shared library directly in `lib/` — verified against the actual
/// `chromium/8044` release archives, not assumed.
pub const PDFIUM_LIBRARY_SUBDIR: &str = "bin";

/// Matches `xtask fetch-onnxruntime`'s `vendor/onnxruntime/<dir>/` layout.
pub const ONNXRUNTIME_VENDOR_DIR: &str = "win-x64";
pub const ONNXRUNTIME_LIBRARY_FILENAME: &str = "onnxruntime.dll";
