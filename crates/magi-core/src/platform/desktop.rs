//! Desktop integration: Dock, clipboard, windowing, backdrop and motion
//! settings.

#[cfg(target_os = "linux")]
use super::linux;
#[cfg(target_os = "macos")]
use super::macos;
#[cfg(target_os = "windows")]
use super::windows;

/// Hides the Dock icon: macOS `Accessory` activation policy, so the app lives
/// in the menu bar like the tray-only app SPEC.md §6.1 asks for. GPUI starts
/// every app as `Regular`; call this once its launch has finished, on the
/// main thread. Nothing elsewhere.
pub fn hide_dock_icon() {
    #[cfg(target_os = "macos")]
    macos::hide_dock_icon();
}

/// Puts `paths` on the clipboard as files, plus `text` for pasting into a
/// text field. `false` when the clipboard refused (on Linux, also when `wl-copy`/`xclip`
/// is missing): the caller then copies `text` alone.
pub fn copy_files(paths: &[std::path::PathBuf], text: &str) -> bool {
    #[cfg(target_os = "windows")]
    {
        windows::copy_files(paths, text)
    }
    #[cfg(target_os = "macos")]
    {
        macos::copy_files(paths, text)
    }
    #[cfg(target_os = "linux")]
    {
        // The text is not set: the clipboard tools own one target each.
        let _ = text;
        linux::copy_files(paths)
    }
}

/// A Windows `CF_HDROP` payload: a `DROPFILES` header (offset 20, no point,
/// wide chars) then each path as NUL-terminated UTF-16, and one more NUL.
/// Pure, so it is tested on every OS.
pub fn dropfiles_bytes(paths: &[std::path::PathBuf]) -> Vec<u8> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&20u32.to_le_bytes());
    bytes.extend_from_slice(&[0; 12]);
    bytes.extend_from_slice(&1u32.to_le_bytes());
    for path in paths {
        for unit in path.to_string_lossy().encode_utf16().chain([0]) {
            bytes.extend_from_slice(&unit.to_le_bytes());
        }
    }
    bytes.extend_from_slice(&[0, 0]);
    bytes
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

/// Whether a borderless pop-up window can take the keyboard. On X11 GPUI's
/// pop-up is override-redirect: no window manager focuses it and GPUI does
/// not either, so search opens as a managed floating window there. GPUI
/// picks Wayland by `WAYLAND_DISPLAY`, so this does too.
pub fn popup_takes_focus() -> bool {
    cfg!(not(target_os = "linux"))
        || std::env::var_os("WAYLAND_DISPLAY").is_some_and(|d| !d.is_empty())
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
pub(super) fn backdrop_override(var: Option<&str>, detected: BackdropSupport) -> BackdropSupport {
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

/// A click on a Windows or Linux (StatusNotifierItem) tray icon opens its
/// app, the menu staying on the right click; a macOS menu-bar icon shows
/// its menu.
pub const TRAY_CLICK_OPENS_APP: bool = cfg!(any(target_os = "windows", target_os = "linux"));

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
