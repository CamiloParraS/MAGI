//! Tray / menu-bar icon (FR-8). The app stays usable without it (SPEC.md §6.3).

use std::sync::Arc;

use gpui_kit::Keystroke;
use magi_core::dto::{IndexState, IndexStatus};
use magi_core::platform::{SYMBOL_MODIFIERS, TRAY_CLICK_OPENS_APP};
use tray_icon::menu::accelerator::Accelerator;
use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

use crate::i18n::Lang;
use crate::onboarding::shortcut_text;
use crate::settings::hotkey_keystroke;
use crate::theme::Tone;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayAction {
    OpenSearch,
    OpenSettings,
    TogglePause,
    Quit,
}

pub fn status_line(status: &IndexStatus, lang: Lang) -> String {
    let s = lang.strings();
    match status.state {
        IndexState::Paused => s.status_paused.into(),
        IndexState::Scanning => s.status_scanning.into(),
        IndexState::Indexing => lang.plural(&s.status_indexing, status.queued),
        IndexState::Idle if status.errors > 0 => lang.plural(&s.status_idle_errors, status.errors),
        IndexState::Idle => s.status_idle.into(),
    }
}

/// The color of [`status_line`]'s dot.
pub fn status_tone(status: &IndexStatus) -> Tone {
    match status.state {
        IndexState::Paused => Tone::Neutral,
        IndexState::Scanning | IndexState::Indexing => Tone::Busy,
        IndexState::Idle if status.errors > 0 => Tone::Err,
        IndexState::Idle => Tone::Ok,
    }
}

pub struct Tray {
    icon: TrayIcon,
    status: MenuItem,
    open: MenuItem,
    pause: MenuItem,
    settings: MenuItem,
    quit: MenuItem,
    lang: Lang,
    /// `ui.hotkey` while it is registered: the tooltip and Open search show it.
    hotkey: Option<String>,
}

impl Tray {
    /// Must run on the main thread with GPUI's event loop running (tray-icon
    /// README). `on_action` runs on tray-icon's event thread.
    pub fn new(
        lang: Lang,
        on_action: impl Fn(TrayAction) + Send + Sync + 'static,
    ) -> Result<Self, String> {
        // Labeled by `set_lang` below.
        let status = MenuItem::new("", false, None);
        let open = MenuItem::new("", true, None);
        let pause = MenuItem::new("", true, None);
        let settings = MenuItem::new("", true, None);
        let quit = MenuItem::new("", true, None);
        let menu = Menu::with_items(&[
            &status,
            &PredefinedMenuItem::separator(),
            &open,
            &pause,
            &settings,
            &PredefinedMenuItem::separator(),
            &quit,
        ])
        .map_err(|e| e.to_string())?;
        let ids = [
            (open.id().clone(), TrayAction::OpenSearch),
            (pause.id().clone(), TrayAction::TogglePause),
            (settings.id().clone(), TrayAction::OpenSettings),
            (quit.id().clone(), TrayAction::Quit),
        ];
        let on_action = Arc::new(on_action);
        let on_menu = on_action.clone();
        MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
            if let Some((_, action)) = ids.iter().find(|(id, _)| *id == event.id) {
                on_menu(*action);
            }
        }));
        // Where a click opens the app, it opens search; the menu stays on
        // the right click.
        if TRAY_CLICK_OPENS_APP {
            TrayIconEvent::set_event_handler(Some(move |event: TrayIconEvent| {
                if let TrayIconEvent::Click {
                    button: MouseButton::Left,
                    button_state: MouseButtonState::Up,
                    ..
                } = event
                {
                    on_action(TrayAction::OpenSearch);
                }
            }));
        }
        let icon = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_menu_on_left_click(!TRAY_CLICK_OPENS_APP)
            .with_icon(placeholder_icon()?)
            .build()
            .map_err(|e| e.to_string())?;
        // Where the OS put it (not known on Linux): the macOS screenshots
        // workflow clicks there.
        if let Some(r) = icon.rect() {
            tracing::info!(
                x = r.position.x,
                y = r.position.y,
                w = r.size.width,
                h = r.size.height,
                "tray icon placed"
            );
        }
        let mut tray = Self {
            icon,
            status,
            open,
            pause,
            settings,
            quit,
            lang,
            hotkey: None,
        };
        tray.set_lang(lang, None);
        Ok(tray)
    }

    /// Relabels every item; `status` is the latest, if any.
    pub fn set_lang(&mut self, lang: Lang, status: Option<&IndexStatus>) {
        let s = lang.strings();
        self.lang = lang;
        self.open.set_text(s.tray_open);
        self.settings.set_text(s.settings);
        self.quit.set_text(s.tray_quit);
        match status {
            Some(status) => self.show_status(status),
            None => {
                self.status.set_text(s.tray_starting);
                self.pause.set_text(s.tray_pause);
            }
        }
        self.show_hotkey();
    }

    /// `None` while the hotkey is not registered: a shortcut that does
    /// nothing is not advertised.
    pub fn set_hotkey(&mut self, hotkey: Option<&str>) {
        self.hotkey = hotkey.map(str::to_owned);
        self.show_hotkey();
    }

    /// The hotkey beside Open search and in the tooltip, so whoever forgot
    /// it finds it where they look for the app.
    fn show_hotkey(&self) {
        let hotkey = self.hotkey.as_deref();
        let accelerator = hotkey.and_then(|h| h.parse::<Accelerator>().ok());
        if let Err(error) = self.open.set_accelerator(accelerator) {
            tracing::warn!(%error, ?hotkey, "the tray could not show the hotkey");
        }
        let tooltip = hotkey
            .and_then(|h| Keystroke::parse(&hotkey_keystroke(h)).ok())
            .map(|k| shortcut_text(&k, self.lang.strings(), SYMBOL_MODIFIERS))
            .map_or_else(|| "Magi".to_owned(), |keys| format!("Magi — {keys}"));
        if let Err(error) = self.icon.set_tooltip(Some(tooltip)) {
            tracing::warn!(%error, "the tray tooltip could not be set");
        }
    }

    pub fn show_status(&self, status: &IndexStatus) {
        let s = self.lang.strings();
        self.status.set_text(status_line(status, self.lang));
        self.pause.set_text(if status.state == IndexState::Paused {
            s.tray_resume
        } else {
            s.tray_pause
        });
    }
}

/// ponytail: a flat 32×32 square until M8 ships the real icon set.
fn placeholder_icon() -> Result<Icon, String> {
    let rgba = [0x3a, 0x6e, 0xd8, 0xff].repeat(32 * 32);
    Icon::from_rgba(rgba, 32, 32).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status(state: IndexState, queued: u64, errors: u64) -> IndexStatus {
        IndexStatus {
            state,
            queued,
            indexed: 0,
            skipped: 0,
            errors,
            current_file: None,
            roots: vec![],
        }
    }

    #[test]
    fn the_status_line_says_what_indexing_is_doing() {
        let line = |state, queued, errors| status_line(&status(state, queued, errors), Lang::En);
        assert_eq!(line(IndexState::Idle, 0, 0), "Up to date");
        assert_eq!(
            line(IndexState::Idle, 0, 3),
            "Up to date, 3 files could not be read"
        );
        assert_eq!(line(IndexState::Scanning, 0, 0), "Scanning folders…");
        assert_eq!(
            line(IndexState::Indexing, 1284, 0),
            "Indexing, 1,284 files to go"
        );
        assert_eq!(line(IndexState::Paused, 9, 0), "Indexing paused");
        assert_eq!(
            status_line(&status(IndexState::Indexing, 1, 0), Lang::Es),
            "Indexando, falta 1 archivo"
        );
    }
}
