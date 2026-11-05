use super::desktop::backdrop_override;
use super::*;
use crate::db::roots::Health;
use crate::dto::Guidance;

#[test]
fn a_drop_list_is_a_header_then_nul_separated_wide_paths() {
    let bytes = dropfiles_bytes(&[Path::new("a").into(), Path::new("bc").into()]);
    let units: Vec<u16> = bytes[20..]
        .chunks(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .collect();
    assert_eq!(&bytes[..4], &20u32.to_le_bytes());
    assert_eq!(&bytes[16..20], &1u32.to_le_bytes());
    assert_eq!(units, [97, 0, 98, 99, 0, 0]);
}

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
    let battery = "Now drawing from 'Battery Power'\n -InternalBattery-0 (id=1)\t80%; discharging";
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
