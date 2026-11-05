//! Per-OS traits: `PermissionProbe`, `CloudPlaceholder`, `PowerStatus`,
//! `ThreadPriority`. Business logic MUST NOT contain `cfg` branches directly;
//! platform-specific code lives in the submodules below (see SPEC.md §6).

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "windows")]
mod windows;

mod desktop;
mod guidance;
mod vendor;

pub use desktop::*;
pub use guidance::*;
pub use vendor::*;

use std::fs::Metadata;
use std::path::Path;

/// Probes whether a root folder is readable, per SPEC.md §6.
pub trait PermissionProbe {
    fn probe(&self, root: &std::path::Path) -> RootAccess;
}

/// Result of a [`PermissionProbe::probe`] call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RootAccess {
    Ok,
    PermissionDenied,
    Missing,
}

/// Lists the folder and stats one entry, which is what a real walk needs.
/// Shared by every OS: the OS-specific part is only the error it returns.
pub struct FsProbe;

impl PermissionProbe for FsProbe {
    fn probe(&self, root: &std::path::Path) -> RootAccess {
        let listed = std::fs::read_dir(root).and_then(|mut entries| match entries.next() {
            Some(entry) => entry?.metadata().map(drop),
            None => Ok(()),
        });
        match listed {
            Ok(()) => RootAccess::Ok,
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                RootAccess::PermissionDenied
            }
            // Not found, not a directory, or a device that is not there.
            Err(_) => RootAccess::Missing,
        }
    }
}

/// Detects cloud-only (dataless) files that must not be read, per SPEC.md §6.
/// Decided from metadata alone: reading content would download the file.
pub trait CloudPlaceholder {
    fn is_cloud_only(&self, metadata: &Metadata) -> bool;
}

/// Reports whether the machine is currently running on battery. `None` when
/// the platform cannot tell (SPEC.md §6.4: the feature is then disabled).
pub trait PowerStatus {
    fn on_battery(&self) -> Option<bool>;
}

/// Lowers the priority of the calling (indexing) thread.
pub trait ThreadPriority {
    fn lower_current_thread(&self);
}

/// The running OS's implementation of the traits above.
pub struct Os;

impl CloudPlaceholder for Os {
    fn is_cloud_only(&self, metadata: &Metadata) -> bool {
        #[cfg(target_os = "windows")]
        {
            windows::is_cloud_only(metadata)
        }
        #[cfg(target_os = "macos")]
        {
            macos::is_cloud_only(metadata)
        }
        #[cfg(target_os = "linux")]
        {
            let _ = metadata;
            false
        }
    }
}

impl PowerStatus for Os {
    fn on_battery(&self) -> Option<bool> {
        #[cfg(target_os = "windows")]
        {
            windows::on_battery()
        }
        #[cfg(target_os = "macos")]
        {
            macos::on_battery()
        }
        #[cfg(target_os = "linux")]
        {
            linux::on_battery()
        }
    }
}

impl ThreadPriority for Os {
    fn lower_current_thread(&self) {
        #[cfg(target_os = "windows")]
        windows::lower_current_thread();
        #[cfg(target_os = "macos")]
        macos::lower_current_thread();
        #[cfg(target_os = "linux")]
        linux::lower_current_thread();
    }
}

/// Windows `FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS | RECALL_ON_OPEN | OFFLINE`
/// (SPEC.md §6.2).
const WINDOWS_CLOUD_ATTRIBUTES: u32 = 0x0040_0000 | 0x0004_0000 | 0x0000_1000;

/// Whether Windows file attributes mark a cloud placeholder. Pure, so it is
/// tested on every OS.
pub fn windows_attributes_cloud_only(attributes: u32) -> bool {
    attributes & WINDOWS_CLOUD_ATTRIBUTES != 0
}

/// macOS `SF_DATALESS`, checked against xnu's `bsd/sys/stat.h`
/// (`#define SF_DATALESS 0x40000000 /* file is dataless object */`).
const SF_DATALESS: u32 = 0x4000_0000;

/// Whether macOS `st_flags` mark a dataless (evicted to iCloud) file.
pub fn macos_flags_dataless(flags: u32) -> bool {
    flags & SF_DATALESS != 0
}

/// Reads `pmset -g batt` output: its first line names the power source,
/// `Now drawing from 'AC Power'` or `'Battery Power'`.
pub fn pmset_on_battery(output: &str) -> Option<bool> {
    let first = output.lines().next()?;
    if first.contains("'Battery Power'") {
        Some(true)
    } else if first.contains("'AC Power'") {
        Some(false)
    } else {
        None
    }
}

/// One `/sys/class/power_supply/*` entry: its `type` and `online` files.
pub struct PowerSupply {
    pub kind: String,
    pub online: Option<bool>,
}

/// On battery when a battery exists and no external supply is online. A
/// machine with no battery is never on battery; with nothing reported at all
/// (a VM, a container) it cannot tell.
pub fn power_supplies_on_battery(supplies: &[PowerSupply]) -> Option<bool> {
    if supplies.is_empty() {
        return None;
    }
    let has_battery = supplies.iter().any(|s| s.kind == "Battery");
    let external_online = supplies
        .iter()
        .any(|s| s.kind != "Battery" && s.online == Some(true));
    Some(has_battery && !external_online)
}

/// Another program holds the file open without sharing, or has locked a range
/// of it: Windows `ERROR_SHARING_VIOLATION` (32) or `ERROR_LOCK_VIOLATION` (33).
/// Worth retrying, and not a reason to give up on the file (SPEC.md §6.2).
pub fn is_locked(error: &std::io::Error) -> bool {
    #[cfg(target_os = "windows")]
    {
        windows::is_locked(error)
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = error;
        false
    }
}

/// A root on a network share or mapped network drive, which is polled instead
/// of watched (SPEC.md §6.2). Windows and macOS tell them apart here; FSEvents
/// reports nothing for SMB/NFS/AFP mounts, so a "watched" one would silently
/// never update.
pub fn is_network_drive(path: &Path) -> bool {
    #[cfg(target_os = "windows")]
    {
        windows::is_network_drive(path)
    }
    #[cfg(target_os = "macos")]
    {
        macos::is_network_drive(path)
    }
    #[cfg(target_os = "linux")]
    {
        let _ = path;
        false
    }
}

/// macOS `statfs` filesystem type names of network mounts. Pure, so it is
/// tested on every OS.
pub fn is_network_fs_type(name: &str) -> bool {
    matches!(name, "smbfs" | "nfs" | "afpfs" | "webdav")
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

#[cfg(test)]
mod tests;
