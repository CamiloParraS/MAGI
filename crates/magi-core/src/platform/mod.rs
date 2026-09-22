//! Per-OS traits: `PermissionProbe`, `CloudPlaceholder`, `PowerStatus`,
//! `ThreadPriority`. Business logic MUST NOT contain `cfg` branches directly;
//! platform-specific code lives in the submodules below (see SPEC.md §6).

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "windows")]
mod windows;

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
/// of watched (SPEC.md §6.2). Only Windows tells them apart here.
pub fn is_network_drive(path: &Path) -> bool {
    #[cfg(target_os = "windows")]
    {
        windows::is_network_drive(path)
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = path;
        false
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probe_tells_a_folder_from_a_missing_one() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(FsProbe.probe(dir.path()), RootAccess::Ok);
        std::fs::write(dir.path().join("a.txt"), "x").unwrap();
        assert_eq!(FsProbe.probe(dir.path()), RootAccess::Ok);
        assert_eq!(FsProbe.probe(&dir.path().join("gone")), RootAccess::Missing);
    }

    #[test]
    fn each_windows_cloud_attribute_marks_a_placeholder() {
        for attr in [0x0040_0000, 0x0004_0000, 0x0000_1000] {
            assert!(windows_attributes_cloud_only(attr | 0x20), "{attr:#x}");
        }
        // ARCHIVE | READONLY | HIDDEN | PINNED (0x80000) is a local file.
        assert!(!windows_attributes_cloud_only(
            0x20 | 0x1 | 0x2 | 0x0008_0000
        ));
    }

    #[test]
    fn only_sf_dataless_marks_a_macos_file_evicted() {
        assert!(macos_flags_dataless(0x4000_0000 | 0x20));
        // UF_HIDDEN (0x8000) and SF_ARCHIVED (0x10000) are not.
        assert!(!macos_flags_dataless(0x8000 | 0x1_0000));
    }

    #[test]
    fn pmset_output_names_the_power_source() {
        let battery =
            "Now drawing from 'Battery Power'\n -InternalBattery-0 (id=1)\t80%; discharging";
        let ac = "Now drawing from 'AC Power'\n -InternalBattery-0 (id=1)\t100%; charged";
        assert_eq!(pmset_on_battery(battery), Some(true));
        assert_eq!(pmset_on_battery(ac), Some(false));
        assert_eq!(pmset_on_battery(""), None);
        assert_eq!(pmset_on_battery("No batteries"), None);
    }

    #[test]
    fn power_supplies_say_battery_only_without_external_power() {
        let supply = |kind: &str, online| PowerSupply {
            kind: kind.into(),
            online,
        };
        let laptop_unplugged = [supply("Battery", None), supply("Mains", Some(false))];
        let laptop_plugged = [supply("Battery", None), supply("Mains", Some(true))];
        let usb_c = [supply("Battery", None), supply("USB", Some(true))];
        let desktop = [supply("Mains", Some(true))];
        assert_eq!(power_supplies_on_battery(&laptop_unplugged), Some(true));
        assert_eq!(power_supplies_on_battery(&laptop_plugged), Some(false));
        assert_eq!(power_supplies_on_battery(&usb_c), Some(false));
        assert_eq!(power_supplies_on_battery(&desktop), Some(false));
        assert_eq!(power_supplies_on_battery(&[]), None);
    }

    #[test]
    fn lowering_thread_priority_does_not_fail() {
        std::thread::spawn(|| Os.lower_current_thread())
            .join()
            .unwrap();
    }
}
