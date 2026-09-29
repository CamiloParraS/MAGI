//! Tray / menu-bar icon (FR-8). English literals until M6 Plan 4 moves them
//! into the string table. The app stays usable without it (SPEC.md §6.3).

use magi_core::dto::{IndexState, IndexStatus};
use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayAction {
    OpenSearch,
    TogglePause,
    Quit,
}

pub fn status_line(status: &IndexStatus) -> String {
    match status.state {
        IndexState::Paused => "Indexing paused".into(),
        IndexState::Scanning => "Scanning folders…".into(),
        IndexState::Indexing => format!("Indexing, {} files to go", status.queued),
        IndexState::Idle if status.errors > 0 => {
            format!("Up to date, {} files could not be read", status.errors)
        }
        IndexState::Idle => "Up to date".into(),
    }
}

pub struct Tray {
    _icon: TrayIcon,
    status: MenuItem,
    pause: MenuItem,
}

impl Tray {
    /// Must run on the main thread with GPUI's event loop running (tray-icon
    /// README). `on_action` runs on tray-icon's event thread.
    pub fn new(on_action: impl Fn(TrayAction) + Send + Sync + 'static) -> Result<Self, String> {
        let status = MenuItem::new("Starting…", false, None);
        let open = MenuItem::new("Open search", true, None);
        let pause = MenuItem::new("Pause indexing", true, None);
        let quit = MenuItem::new("Quit", true, None);
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
        })
    }

    pub fn show_status(&self, status: &IndexStatus) {
        self.status.set_text(status_line(status));
        self.pause.set_text(if status.state == IndexState::Paused {
            "Resume indexing"
        } else {
            "Pause indexing"
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
        assert_eq!(status_line(&status(IndexState::Idle, 0, 0)), "Up to date");
        assert_eq!(
            status_line(&status(IndexState::Idle, 0, 3)),
            "Up to date, 3 files could not be read"
        );
        assert_eq!(
            status_line(&status(IndexState::Scanning, 0, 0)),
            "Scanning folders…"
        );
        assert_eq!(
            status_line(&status(IndexState::Indexing, 1284, 0)),
            "Indexing, 1284 files to go"
        );
        assert_eq!(
            status_line(&status(IndexState::Paused, 9, 0)),
            "Indexing paused"
        );
    }
}
