//! Windows implementations of the `platform` traits (see SPEC.md §6.2).

/// Matches `xtask`'s `vendor/pdfium/<dir>/` layout (see `xtask/src/main.rs`).
pub const PDFIUM_VENDOR_DIR: &str = "win-x64";
pub const PDFIUM_LIBRARY_FILENAME: &str = "pdfium.dll";
