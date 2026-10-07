//! The system-wide search hotkey (FR-7): registering it, and turning a
//! keystroke pressed in settings into a new `ui.hotkey`.

use global_hotkey::hotkey::HotKey;
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use gpui_kit::Keystroke;

/// Accepts the Tauri-style strings the config uses (`CmdOrCtrl+Shift+Space`).
pub fn parse(spec: &str) -> Result<HotKey, String> {
    spec.parse::<HotKey>().map_err(|e| e.to_string())
}

/// A keystroke pressed in the settings recorder as a `ui.hotkey` value, or
/// `None` until it is a usable shortcut: Ctrl, Alt or Cmd/Win plus one key
/// `global-hotkey` knows (Shift alone would steal typing).
pub fn from_keystroke(keystroke: &Keystroke) -> Option<String> {
    let m = &keystroke.modifiers;
    if !(m.control || m.alt || m.platform) {
        return None;
    }
    let mut key = keystroke.key.clone();
    if let Some(first) = key.get_mut(..1) {
        first.make_ascii_uppercase();
    }
    let spec = [
        (m.control, "Ctrl"),
        (m.alt, "Alt"),
        (m.shift, "Shift"),
        (m.platform, "Super"),
    ]
    .into_iter()
    .filter_map(|(on, name)| on.then_some(name))
    .chain([key.as_str()])
    .collect::<Vec<_>>()
    .join("+");
    // A modifier still held parses as a key name `global-hotkey` rejects.
    parse(&spec).is_ok().then_some(spec)
}

/// Combos the OS keeps for itself even though registering them succeeds:
/// Alt+Space opens the Windows window menu (SPEC.md §5.2), Cmd/Win+Space
/// is Spotlight or the input switcher.
pub fn reserved(spec: &str) -> bool {
    parse(spec).is_ok_and(|hotkey| {
        ["Alt+Space", "Super+Space"]
            .iter()
            .any(|r| parse(r) == Ok(hotkey))
    })
}

/// The registered search hotkey. `global-hotkey` needs the event loop's
/// thread: create and change it inside GPUI's `run`. Dropping it
/// unregisters the hotkey.
pub struct Hotkey {
    manager: GlobalHotKeyManager,
    current: Option<HotKey>,
}

impl Hotkey {
    /// `on_press` runs on `global-hotkey`'s event thread.
    pub fn new(on_press: impl Fn() + Send + Sync + 'static) -> Result<Self, String> {
        let manager = GlobalHotKeyManager::new().map_err(|e| e.to_string())?;
        // Only one hotkey is registered at a time, so any press is it.
        GlobalHotKeyEvent::set_event_handler(Some(move |event: GlobalHotKeyEvent| {
            if matches!(event.state, HotKeyState::Pressed) {
                on_press();
            }
        }));
        Ok(Self {
            manager,
            current: None,
        })
    }

    /// Registers `spec` in place of the current hotkey. Fails when another
    /// app or the OS holds it (FR-7 conflict detection); the current one
    /// then stays.
    pub fn set(&mut self, spec: &str) -> Result<(), String> {
        let hotkey = parse(spec)?;
        if self.current == Some(hotkey) {
            return Ok(());
        }
        self.manager.register(hotkey).map_err(|e| e.to_string())?;
        if let Some(old) = self.current.replace(hotkey) {
            let _ = self.manager.unregister(old);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_hotkey_parses() {
        let default = magi_core::config::UiConfig::default().hotkey;
        assert!(parse(&default).is_ok(), "{default}");
    }

    fn recorded(key: &str) -> Option<String> {
        from_keystroke(&gpui_kit::Keystroke::parse(key).unwrap())
    }

    #[test]
    fn a_recorded_keystroke_becomes_a_hotkey_setting() {
        assert_eq!(
            recorded("ctrl-shift-space").as_deref(),
            Some("Ctrl+Shift+Space")
        );
        assert_eq!(recorded("alt-k").as_deref(), Some("Alt+K"));
        assert_eq!(recorded("cmd-f5").as_deref(), Some("Super+F5"));
        assert_eq!(recorded("ctrl-alt-,").as_deref(), Some("Ctrl+Alt+,"));
        for spec in ["Ctrl+Shift+Space", "Alt+K", "Super+F5", "Ctrl+Alt+,"] {
            assert!(parse(spec).is_ok(), "{spec}");
        }
    }

    #[test]
    fn a_recording_needs_ctrl_alt_or_cmd_and_a_real_key() {
        assert_eq!(recorded("k"), None, "plain typing");
        assert_eq!(recorded("shift-k"), None, "shift alone is typing too");
        assert_eq!(recorded("ctrl-shift"), None, "modifiers only, still held");
        assert_eq!(recorded("ctrl-+"), None, "not a key global-hotkey knows");
    }

    #[test]
    fn combos_the_system_keeps_are_reserved() {
        assert!(reserved("Alt+Space"), "the Windows window menu");
        assert!(reserved("Super+Space"), "Spotlight / input switching");
        assert!(!reserved("Ctrl+Shift+Space"));
        assert!(!reserved("not a hotkey"));
    }

    #[test]
    fn an_unparsable_hotkey_is_an_error_not_a_panic() {
        assert!(parse("Ctrl+Shift+").is_err());
        assert!(parse("Hyper+Space").is_err());
    }
}
