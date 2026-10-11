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

use crate::db::roots::Health;
use crate::dto::Guidance;

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

/// Hides the Dock icon: macOS `Accessory` activation policy, so the app lives
/// in the menu bar like the tray-only app SPEC.md §6.1 asks for. GPUI starts
/// every app as `Regular`; call this once its launch has finished, on the
/// main thread. Nothing elsewhere.
pub fn hide_dock_icon() {
    #[cfg(target_os = "macos")]
    macos::hide_dock_icon();
}

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

/// A Wayland session, where apps may not get a global shortcut; the user
/// binds `magi --toggle` in the desktop instead (SPEC.md §6.3).
pub fn is_wayland() -> bool {
    #[cfg(target_os = "linux")]
    {
        std::env::var("XDG_SESSION_TYPE").is_ok_and(|t| t == "wayland")
    }
    #[cfg(not(target_os = "linux"))]
    {
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

/// What the OS offers the search window's background (ADR-0011).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct BackdropSupport {
    /// GPUI's `Blurred` background really blurs: Mica on Windows 11 22H2+
    /// (`DWMWA_SYSTEMBACKDROP_TYPE`, which older builds ignore), an
    /// `NSVisualEffectView` on macOS.
    pub blur: bool,
    /// The user turned transparency effects off in the OS.
    pub reduce_transparency: bool,
}

/// GPUI's Windows backend applies Mica only from build 22621 (gpui-pre 0.3.7).
pub fn mica_supported_on_build(build: u32) -> bool {
    build >= 22621
}

/// This machine's [`BackdropSupport`]: Windows 11 22H2+ and macOS blur, Linux
/// doesn't. `MAGI_BACKDROP=blurred` forces the blur (SPEC.md §4.5), for
/// trying it where it is off.
pub fn backdrop_support() -> BackdropSupport {
    #[cfg(target_os = "windows")]
    let detected = windows::backdrop_support();
    #[cfg(target_os = "macos")]
    let detected = BackdropSupport {
        blur: true,
        reduce_transparency: read_setting(
            "defaults",
            &["read", "com.apple.universalaccess", "reduceTransparency"],
        ) == Some(true),
    };
    #[cfg(target_os = "linux")]
    let detected = BackdropSupport::default();
    backdrop_override(std::env::var("MAGI_BACKDROP").ok().as_deref(), detected)
}

/// `MAGI_BACKDROP` over what the OS offers: `blurred` turns the blur on and
/// ignores the OS's reduce-transparency setting; anything else changes
/// nothing.
fn backdrop_override(var: Option<&str>, detected: BackdropSupport) -> BackdropSupport {
    match var {
        Some("blurred") => BackdropSupport {
            blur: true,
            reduce_transparency: false,
        },
        _ => detected,
    }
}

/// macOS shows modifier keys as symbols (⌘ ⌥ ⇧ ⌃); Windows and Linux by
/// name (Ctrl, Alt, Shift).
pub const SYMBOL_MODIFIERS: bool = cfg!(target_os = "macos");

/// A click on a Windows tray icon opens its app; a macOS menu-bar icon
/// shows its menu, and Linux's AppIndicator reports no clicks at all.
pub const TRAY_CLICK_OPENS_APP: bool = cfg!(target_os = "windows");

/// The user asked the OS for less motion: Windows' "Show animations" off,
/// macOS "Reduce motion", GNOME animations off. `false` when unknown.
pub fn reduce_motion() -> bool {
    #[cfg(target_os = "windows")]
    {
        windows::reduce_motion()
    }
    #[cfg(target_os = "macos")]
    {
        read_setting(
            "defaults",
            &["read", "com.apple.universalaccess", "reduceMotion"],
        ) == Some(true)
    }
    #[cfg(target_os = "linux")]
    {
        read_setting(
            "gsettings",
            &["get", "org.gnome.desktop.interface", "enable-animations"],
        ) == Some(false)
    }
}

/// ponytail: one process spawn, once at startup; a native API if it ever
/// needs to follow the setting live.
#[cfg(not(target_os = "windows"))]
fn read_setting(program: &str, args: &[&str]) -> Option<bool> {
    let output = std::process::Command::new(program)
        .args(args)
        .output()
        .ok()?;
    setting_flag(&String::from_utf8_lossy(&output.stdout))
}

/// A boolean setting as `defaults` (`1`/`0`) or `gsettings` (`true`/`false`) prints it.
pub fn setting_flag(output: &str) -> Option<bool> {
    match output.trim() {
        "1" | "true" => Some(true),
        "0" | "false" => Some(false),
        _ => None,
    }
}

/// The OS id of the display under the mouse cursor, which is what GPUI's
/// `DisplayId` wraps: the `HMONITOR` on Windows, the `CGDirectDisplayID` on
/// macOS (gpui-pre 0.3.7). `None` on Linux, where Wayland has no global
/// cursor position, or when the OS call fails.
pub fn display_under_cursor() -> Option<u64> {
    #[cfg(target_os = "windows")]
    {
        windows::display_under_cursor()
    }
    #[cfg(target_os = "macos")]
    {
        macos::display_under_cursor()
    }
    #[cfg(target_os = "linux")]
    {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_denied_macos_folder_points_at_the_right_privacy_page() {
        let home = Path::new("/Users/ana");
        let mac = |path: &str| {
            guidance(
                OsKind::MacOs,
                Health::PermissionDenied,
                Path::new(path),
                Some(home),
                None,
            )
        };
        for consent in [
            "/Users/ana/Documents",
            "/Users/ana/Desktop/notes",
            "/Users/ana/Downloads",
            "/Users/ana/Library/Mobile Documents/com~apple~CloudDocs",
            "/Volumes/USB",
        ] {
            assert_eq!(
                mac(consent),
                Some(Guidance::MacFilesAndFolders),
                "{consent}"
            );
        }
        for other in ["/Users/ana", "/Users/ana/DocumentsOld", "/Library/Mail"] {
            assert_eq!(mac(other), Some(Guidance::MacFullDiskAccess), "{other}");
        }
        assert_eq!(
            Guidance::MacFilesAndFolders.settings_url(),
            Some("x-apple.systempreferences:com.apple.preference.security?Privacy_FilesAndFolders")
        );
        assert_eq!(
            Guidance::MacFullDiskAccess.settings_url(),
            Some("x-apple.systempreferences:com.apple.preference.security?Privacy_AllFiles")
        );
    }

    #[test]
    fn each_os_explains_a_denied_polled_or_missing_root() {
        let at = |os, status| guidance(os, status, Path::new("/data"), None, Some(8192));
        assert_eq!(at(OsKind::Windows, Health::Ok), None);
        assert_eq!(
            at(OsKind::Windows, Health::PermissionDenied),
            Some(Guidance::AccessDenied)
        );
        assert_eq!(
            at(OsKind::Linux, Health::PermissionDenied),
            Some(Guidance::AccessDenied)
        );
        assert_eq!(
            at(OsKind::Linux, Health::WatchFailed),
            Some(Guidance::WatchLimit { limit: Some(8192) })
        );
        assert_eq!(
            at(OsKind::MacOs, Health::WatchFailed),
            Some(Guidance::Polled)
        );
        assert_eq!(
            at(OsKind::Windows, Health::WatchFailed),
            Some(Guidance::Polled)
        );
        assert_eq!(at(OsKind::Linux, Health::Missing), Some(Guidance::Missing));
        assert_eq!(Guidance::AccessDenied.settings_url(), None);
    }

    #[test]
    fn magi_backdrop_blurred_forces_the_blur_anywhere() {
        let solid = BackdropSupport {
            blur: false,
            reduce_transparency: true,
        };
        let forced = BackdropSupport {
            blur: true,
            reduce_transparency: false,
        };
        assert_eq!(backdrop_override(Some("blurred"), solid), forced);
        // Unset or anything else: what the OS offers.
        assert_eq!(backdrop_override(None, solid), solid);
        assert_eq!(backdrop_override(Some("1"), solid), solid);
    }

    #[test]
    fn mica_needs_windows_11_22h2() {
        assert!(!mica_supported_on_build(19045)); // Windows 10 22H2
        assert!(!mica_supported_on_build(22000)); // Windows 11 21H2
        assert!(mica_supported_on_build(22621));
        assert!(mica_supported_on_build(26100));
    }

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
    fn macos_network_mounts_are_polled_and_local_disks_watched() {
        for network in ["smbfs", "nfs", "afpfs", "webdav"] {
            assert!(is_network_fs_type(network), "{network}");
        }
        // APFS, HFS+, exFAT/FAT (USB sticks) and the disk-image filesystem.
        for local in ["apfs", "hfs", "exfat", "msdos", "ntfs", ""] {
            assert!(!is_network_fs_type(local), "{local}");
        }
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
    fn setting_output_reads_as_a_flag() {
        // `defaults read … reduceMotion` and `gsettings get … enable-animations`.
        assert_eq!(setting_flag("1\n"), Some(true));
        assert_eq!(setting_flag("0\n"), Some(false));
        assert_eq!(setting_flag("true\n"), Some(true));
        assert_eq!(setting_flag("false\n"), Some(false));
        assert_eq!(setting_flag(""), None);
        assert_eq!(setting_flag("No such key"), None);
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
