//! The system-wide search hotkey (FR-7): registering it, and turning a
//! keystroke pressed in settings into a new `ui.hotkey`.

use global_hotkey::hotkey::HotKey;
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use gpui_kit::{BackgroundExecutor, Keystroke};
use magi_core::config::Config;
use magi_core::host::Host;
use serde_json::json;

use crate::app::{AppEvent, Events};

/// Accepts the Tauri-style strings the config uses (`CmdOrCtrl+Shift+Space`).
pub fn parse(spec: &str) -> Result<HotKey, String> {
    spec.parse::<HotKey>().map_err(|e| e.to_string())
}

/// `ui.hotkey` (global-hotkey's `CmdOrCtrl+Shift+Space`) as a GPUI
/// keystroke, which [`Kbd`](gpui_kit::component::kbd::Kbd) shows the
/// platform's way.
pub fn to_keystroke(hotkey: &str) -> String {
    hotkey
        .split('+')
        .map(|key| {
            let key = key.to_lowercase();
            match key.as_str() {
                "cmdorctrl" | "cmdorcontrol" | "commandorctrl" | "commandorcontrol" => {
                    "secondary".into()
                }
                "control" => "ctrl".into(),
                "option" => "alt".into(),
                "command" | "super" => "cmd".into(),
                _ => key
                    .strip_prefix("key")
                    .or_else(|| key.strip_prefix("digit"))
                    .filter(|rest| rest.len() == 1)
                    .map_or_else(|| key.clone(), String::from),
            }
        })
        .collect::<Vec<_>>()
        .join("-")
}

/// What a key pressed in a shortcut recorder does.
#[derive(Debug, PartialEq, Eq)]
pub enum RecorderStep {
    Cancel,
    /// A modifier still held, or plain typing.
    Wait,
    Submit(String),
}

pub fn recorder_step(keystroke: &Keystroke) -> RecorderStep {
    if keystroke.key == "escape" && !keystroke.modifiers.modified() {
        return RecorderStep::Cancel;
    }
    from_keystroke(keystroke).map_or(RecorderStep::Wait, RecorderStep::Submit)
}

/// Asks the shell to register `spec` in place of the hotkey (FR-7 conflict
/// detection); true when it did.
async fn register(events: &Events, spec: String) -> bool {
    let (reply, registered) = async_channel::bounded(1);
    events.send(AppEvent::Hotkey(spec, reply)).await.is_ok() && registered.recv().await == Ok(true)
}

/// Registers `spec`, then saves it only if that worked. `None` when it was
/// refused (the shell has recorded the conflict); the caller publishes a
/// saved config.
///
/// ponytail: a failed save leaves the new hotkey registered until restart;
/// re-register the old one if that ever matters.
pub async fn change(
    events: &Events,
    host: Host,
    spec: String,
    executor: &BackgroundExecutor,
) -> Option<magi_core::Result<Config>> {
    if !register(events, spec.clone()).await {
        return None;
    }
    let patch = json!({ "ui": { "hotkey": spec } });
    Some(
        executor
            .spawn(async move { host.update_settings(&patch) })
            .await,
    )
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

    fn step(key: &str) -> RecorderStep {
        recorder_step(&gpui_kit::Keystroke::parse(key).unwrap())
    }

    #[test]
    fn the_recorder_cancels_on_escape_and_waits_for_a_usable_shortcut() {
        assert_eq!(step("escape"), RecorderStep::Cancel);
        assert_eq!(step("shift-k"), RecorderStep::Wait);
        assert_eq!(
            step("ctrl-escape"),
            RecorderStep::Submit("Ctrl+Escape".into()),
            "only a bare Esc cancels"
        );
        assert_eq!(step("alt-k"), RecorderStep::Submit("Alt+K".into()));
    }

    #[test]
    fn the_hotkey_setting_becomes_a_gpui_keystroke() {
        assert_eq!(
            to_keystroke("CmdOrCtrl+Shift+Space"),
            "secondary-shift-space"
        );
        assert_eq!(to_keystroke("Alt+K"), "alt-k");
        assert_eq!(
            to_keystroke("Control+Option+KeyK"),
            "ctrl-alt-k",
            "global-hotkey's other spellings"
        );
        assert_eq!(to_keystroke("CommandOrControl+Digit1"), "secondary-1");
        assert_eq!(to_keystroke("Command+Space"), "cmd-space");
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
