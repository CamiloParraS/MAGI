//! What to tell the user about a root that is not fine, per OS (SPEC.md §6.1).

use std::path::Path;

use crate::db::roots::Health;
use crate::dto::Guidance;

/// Which OS's guidance to give. A value, so every OS's choice is tested on
/// any OS.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OsKind {
    Windows,
    MacOs,
    Linux,
}

#[cfg(target_os = "windows")]
pub const CURRENT_OS: OsKind = OsKind::Windows;
#[cfg(target_os = "macos")]
pub const CURRENT_OS: OsKind = OsKind::MacOs;
#[cfg(target_os = "linux")]
pub const CURRENT_OS: OsKind = OsKind::Linux;

/// System Settings pages for a denied folder (SPEC.md §6.1). Unverified on a
/// real macOS 13, 14 or 15 yet (`docs/permissions.md`).
pub const MACOS_FILES_AND_FOLDERS_URL: &str =
    "x-apple.systempreferences:com.apple.preference.security?Privacy_FilesAndFolders";
pub const MACOS_FULL_DISK_ACCESS_URL: &str =
    "x-apple.systempreferences:com.apple.preference.security?Privacy_AllFiles";

/// What to tell the user about a root in `status` on `os`; `None` when it is
/// fine. `home` places macOS's consent folders; `watch_limit` is the inotify
/// limit, quoted in Linux's advice.
pub fn guidance(
    os: OsKind,
    status: Health,
    path: &Path,
    home: Option<&Path>,
    watch_limit: Option<u64>,
) -> Option<Guidance> {
    Some(match (status, os) {
        (Health::Ok, _) => return None,
        (Health::Missing, _) => Guidance::Missing,
        (Health::PermissionDenied, OsKind::MacOs) if macos_consent_folder(path, home) => {
            Guidance::MacFilesAndFolders
        }
        (Health::PermissionDenied, OsKind::MacOs) => Guidance::MacFullDiskAccess,
        (Health::PermissionDenied, _) => Guidance::AccessDenied,
        // Linux falls back to polling when inotify runs out of watches;
        // elsewhere only network drives are polled.
        (Health::WatchFailed, OsKind::Linux) => Guidance::WatchLimit { limit: watch_limit },
        (Health::WatchFailed, _) => Guidance::Polled,
    })
}

/// Inside a folder macOS asks consent for in Files and Folders: Desktop,
/// Documents, Downloads, iCloud Drive, or a removable or network volume.
/// Anything else denied needs Full Disk Access.
fn macos_consent_folder(path: &Path, home: Option<&Path>) -> bool {
    let in_home = home.is_some_and(|home| {
        [
            "Desktop",
            "Documents",
            "Downloads",
            "Library/Mobile Documents",
        ]
        .iter()
        .any(|dir| path.starts_with(home.join(dir)))
    });
    in_home || path.starts_with("/Volumes")
}
