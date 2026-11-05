//! Where the vendored native libraries (PDFium, ONNX Runtime) live for the
//! current OS/arch.

#[cfg(target_os = "linux")]
use super::linux;
#[cfg(target_os = "macos")]
use super::macos;
#[cfg(target_os = "windows")]
use super::windows;

/// The `vendor/pdfium/<dir>/` subdirectory name for the current OS/arch,
/// matching `xtask`'s fetch targets.
pub fn pdfium_vendor_dir() -> &'static str {
    #[cfg(target_os = "windows")]
    {
        windows::PDFIUM_VENDOR_DIR
    }
    #[cfg(target_os = "macos")]
    {
        macos::PDFIUM_VENDOR_DIR
    }
    #[cfg(target_os = "linux")]
    {
        linux::PDFIUM_VENDOR_DIR
    }
}

/// The PDFium shared library's filename for the current OS.
pub fn pdfium_library_filename() -> &'static str {
    #[cfg(target_os = "windows")]
    {
        windows::PDFIUM_LIBRARY_FILENAME
    }
    #[cfg(target_os = "macos")]
    {
        macos::PDFIUM_LIBRARY_FILENAME
    }
    #[cfg(target_os = "linux")]
    {
        linux::PDFIUM_LIBRARY_FILENAME
    }
}

/// The subdirectory (relative to `vendor/pdfium/<dir>/`) holding the
/// PDFium shared library on the current OS: `bin` on Windows, `lib` on
/// macOS/Linux — the pdfium-binaries releases lay these out differently.
pub fn pdfium_library_subdir() -> &'static str {
    #[cfg(target_os = "windows")]
    {
        windows::PDFIUM_LIBRARY_SUBDIR
    }
    #[cfg(target_os = "macos")]
    {
        macos::PDFIUM_LIBRARY_SUBDIR
    }
    #[cfg(target_os = "linux")]
    {
        linux::PDFIUM_LIBRARY_SUBDIR
    }
}

/// The `vendor/onnxruntime/<dir>/` subdirectory name for the current
/// OS/arch, matching `xtask fetch-onnxruntime`'s targets.
pub fn onnxruntime_vendor_dir() -> &'static str {
    #[cfg(target_os = "windows")]
    {
        windows::ONNXRUNTIME_VENDOR_DIR
    }
    #[cfg(target_os = "macos")]
    {
        macos::ONNXRUNTIME_VENDOR_DIR
    }
    #[cfg(target_os = "linux")]
    {
        linux::ONNXRUNTIME_VENDOR_DIR
    }
}

/// The ONNX Runtime shared library's filename for the current OS. Always
/// under a `lib/` subdirectory (unlike PDFium, the official onnxruntime
/// releases lay this out the same way on all three OSes).
pub fn onnxruntime_library_filename() -> &'static str {
    #[cfg(target_os = "windows")]
    {
        windows::ONNXRUNTIME_LIBRARY_FILENAME
    }
    #[cfg(target_os = "macos")]
    {
        macos::ONNXRUNTIME_LIBRARY_FILENAME
    }
    #[cfg(target_os = "linux")]
    {
        linux::ONNXRUNTIME_LIBRARY_FILENAME
    }
}
