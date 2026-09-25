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

use std::os::windows::fs::MetadataExt;
use std::path::{Component, Path, Prefix};

use windows_sys::Win32::Storage::FileSystem::GetDriveTypeW;
use windows_sys::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};
use windows_sys::Win32::System::Threading::{
    GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_BELOW_NORMAL,
};
use windows_sys::Win32::System::WindowsProgramming::DRIVE_REMOTE;

/// Attributes come from the directory entry: reading them never hydrates.
pub fn is_cloud_only(metadata: &std::fs::Metadata) -> bool {
    super::windows_attributes_cloud_only(metadata.file_attributes())
}

pub fn is_locked(error: &std::io::Error) -> bool {
    const ERROR_SHARING_VIOLATION: i32 = 32;
    const ERROR_LOCK_VIOLATION: i32 = 33;
    matches!(
        error.raw_os_error(),
        Some(ERROR_SHARING_VIOLATION | ERROR_LOCK_VIOLATION)
    )
}

/// A UNC path (`\\server\share`), or a drive letter mapped to one.
pub fn is_network_drive(path: &Path) -> bool {
    let Some(Component::Prefix(prefix)) = path.components().next() else {
        return false;
    };
    let letter = match prefix.kind() {
        Prefix::UNC(..) | Prefix::VerbatimUNC(..) => return true,
        Prefix::Disk(letter) | Prefix::VerbatimDisk(letter) => letter,
        _ => return false,
    };
    let root: Vec<u16> = format!("{}:\\", char::from(letter))
        .encode_utf16()
        .chain([0])
        .collect();
    // SAFETY: `root` is a NUL-terminated UTF-16 string that outlives the call.
    unsafe { GetDriveTypeW(root.as_ptr()) == DRIVE_REMOTE }
}

pub fn on_battery() -> Option<bool> {
    let mut status: SYSTEM_POWER_STATUS = unsafe { std::mem::zeroed() };
    // SAFETY: `status` is a valid, writable SYSTEM_POWER_STATUS.
    if unsafe { GetSystemPowerStatus(&mut status) } == 0 {
        return None;
    }
    // ACLineStatus: 0 offline, 1 online, 255 unknown.
    match status.ACLineStatus {
        0 => Some(true),
        1 => Some(false),
        _ => None,
    }
}

pub fn lower_current_thread() {
    // SAFETY: the pseudo-handle of the calling thread is always valid.
    if unsafe { SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_BELOW_NORMAL) } == 0 {
        tracing::debug!(error = %std::io::Error::last_os_error(), "could not lower thread priority");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::{CloudPlaceholder, Os};
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{FILE_ATTRIBUTE_OFFLINE, SetFileAttributesW};

    /// A real attribute on a real file, read back through the walker's
    /// metadata: `OFFLINE` is the one cloud attribute a test can set.
    #[test]
    fn a_file_marked_offline_is_cloud_only() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("placeholder.txt");
        std::fs::write(&path, "not here").unwrap();
        assert!(!Os.is_cloud_only(&std::fs::metadata(&path).unwrap()));

        let wide: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
        assert_ne!(
            unsafe { SetFileAttributesW(wide.as_ptr(), FILE_ATTRIBUTE_OFFLINE) },
            0
        );
        assert!(Os.is_cloud_only(&std::fs::metadata(&path).unwrap()));
        let walked = crate::discovery::stat(&path).unwrap();
        assert!(walked.cloud_only);
    }

    #[test]
    fn unc_paths_are_network_drives_and_temp_is_not() {
        assert!(is_network_drive(Path::new(r"\\server\share\docs")));
        assert!(is_network_drive(Path::new(r"\\?\UNC\server\share")));
        assert!(!is_network_drive(&std::env::temp_dir()));
    }

    #[test]
    fn sharing_and_lock_violations_are_locks() {
        assert!(is_locked(&std::io::Error::from_raw_os_error(32)));
        assert!(is_locked(&std::io::Error::from_raw_os_error(33)));
        assert!(!is_locked(&std::io::Error::from_raw_os_error(5)));
    }
}
