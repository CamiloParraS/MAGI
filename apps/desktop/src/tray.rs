//! Tray / menu-bar icon (FR-8). The app stays usable without it (SPEC.md §6.3).

use magi_core::dto::{IndexState, IndexStatus};
use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};

use crate::i18n::Lang;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayAction {
    OpenSearch,
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

pub struct Tray {
    _icon: TrayIcon,
    status: MenuItem,
    pause: MenuItem,
    lang: Lang,
}

impl Tray {
    /// Must run on the main thread with GPUI's event loop running (tray-icon
    /// README). `on_action` runs on tray-icon's event thread.
    /// ponytail: `lang` is fixed at startup; the settings window (Plan 5)
    /// switches it live by rebuilding the tray or setting each item's text.
    pub fn new(
        lang: Lang,
        on_action: impl Fn(TrayAction) + Send + Sync + 'static,
    ) -> Result<Self, String> {
        let s = lang.strings();
        let status = MenuItem::new(s.tray_starting, false, None);
        let open = MenuItem::new(s.tray_open, true, None);
        let pause = MenuItem::new(s.tray_pause, true, None);
        let quit = MenuItem::new(s.tray_quit, true, None);
        let menu = Menu::with_items(&[
            &status,
            &PredefinedMenuItem::separator(),
            &open,
            &pause,
            &PredefinedMenuItem::separator(),
            &quit,
        ])
        .map_err(|e| e.to_string())?;
        let ids = [
            (open.id().clone(), TrayAction::OpenSearch),
            (pause.id().clone(), TrayAction::TogglePause),
            (quit.id().clone(), TrayAction::Quit),
        ];
        MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
            if let Some((_, action)) = ids.iter().find(|(id, _)| *id == event.id) {
                on_action(*action);
            }
        }));
        let icon = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_icon(placeholder_icon()?)
            .with_tooltip("magi")
            .build()
            .map_err(|e| e.to_string())?;
        Ok(Self {
            _icon: icon,
            status,
            pause,
            lang,
        })
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
