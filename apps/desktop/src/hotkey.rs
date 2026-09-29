//! The system-wide search hotkey (FR-7). `global-hotkey` needs the event
//! loop's thread: call [`register`] inside GPUI's `run`.

use global_hotkey::hotkey::HotKey;
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};

/// Accepts the Tauri-style strings the config uses (`CmdOrCtrl+Shift+Space`).
pub fn parse(spec: &str) -> Result<HotKey, String> {
    spec.parse::<HotKey>().map_err(|e| e.to_string())
}

/// Registers `spec`; `on_press` runs on `global-hotkey`'s event thread.
pub fn register(
    spec: &str,
    on_press: impl Fn() + Send + Sync + 'static,
) -> Result<GlobalHotKeyManager, String> {
    let hotkey = parse(spec)?;
    let id = hotkey.id();
    let manager = GlobalHotKeyManager::new().map_err(|e| e.to_string())?;
    manager.register(hotkey).map_err(|e| e.to_string())?;
    GlobalHotKeyEvent::set_event_handler(Some(move |event: GlobalHotKeyEvent| {
        if event.id == id && matches!(event.state, HotKeyState::Pressed) {
            on_press();
        }
    }));
    Ok(manager)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_hotkey_parses() {
        let default = magi_core::config::UiConfig::default().hotkey;
        assert!(parse(&default).is_ok(), "{default}");
    }

    #[test]
    fn an_unparsable_hotkey_is_an_error_not_a_panic() {
        assert!(parse("Ctrl+Shift+").is_err());
        assert!(parse("Hyper+Space").is_err());
    }
}
