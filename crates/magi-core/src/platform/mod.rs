//! Per-OS traits: `PermissionProbe`, `CloudPlaceholder`, `PowerStatus`,
//! `ThreadPriority`. Business logic MUST NOT contain `cfg` branches directly;
//! platform-specific code lives in the submodules below (see SPEC.md §6).

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "windows")]
mod windows;

/// Probes whether a root folder is readable, per SPEC.md §6.
pub trait PermissionProbe {
    fn probe(&self, root: &std::path::Path) -> RootAccess;
}

/// Result of a [`PermissionProbe::probe`] call.
pub enum RootAccess {
    Ok,
    PermissionDenied,
    Missing,
}

/// Detects cloud-only (dataless) files that must not be read, per SPEC.md §6.
pub trait CloudPlaceholder {
    fn is_cloud_only(&self, path: &std::path::Path) -> bool;
}

/// Reports whether the machine is currently running on battery.
pub trait PowerStatus {
    fn on_battery(&self) -> Option<bool>;
}

/// Lowers the priority of the calling (indexing) thread.
pub trait ThreadPriority {
    fn lower_current_thread(&self);
}

/// The kernel's inotify watch limit, for `magi-cli doctor`. `None` on
/// platforms without inotify.
pub fn inotify_watch_limit() -> Option<u64> {
    #[cfg(target_os = "linux")]
    {
        linux::inotify_watch_limit()
    }
    #[cfg(not(target_os = "linux"))]
    {
        None
    }
}

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
