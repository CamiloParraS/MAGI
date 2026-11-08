//! Opening, closing and bringing forward the app's three windows.

use std::time::Instant;

use gpui_kit::*;

use super::Shell;
use crate::live::Live;
use crate::onboarding::{self, OnboardingView};
use crate::search::view::{self, Opening, SearchView};
use crate::settings::view::{self as settings, SettingsView};
use crate::theme;

impl Shell {
    /// GPUI cannot hide a window on Windows (`cx.hide()` is a no-op there),
    /// so the search window is closed and reopened instead (ADR-0011).
    pub(super) fn close_window(&mut self, cx: &mut App) -> bool {
        self.window
            .take()
            .is_some_and(|w| w.update(cx, |_, window, _| window.remove_window()).is_ok())
    }

    /// A normal window with the search window's background (SPEC M6).
    pub(super) fn open_settings(&mut self, cx: &mut App) {
        let backdrop = theme::backdrop(
            self.ui.transparency_mode,
            self.ui.transparency_intensity,
            magi_core::platform::backdrop_support(),
        );
        let options = WindowOptions {
            titlebar: Some(TitlebarOptions {
                title: Some(self.live.read(cx).lang.strings().settings.into()),
                ..Default::default()
            }),
            window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                None,
                settings::SIZE,
                cx,
            ))),
            window_min_size: Some(settings::MIN_SIZE),
            focus: true,
            show: true,
            window_background: theme::window_background(backdrop),
            ..Default::default()
        };
        let (host, live, events) = (self.host.clone(), self.live.clone(), self.events.clone());
        match gpui_kit::open_window(options, cx, move |window, cx| {
            cx.new(|cx| SettingsView::new(host, live, events, window, cx))
        }) {
            Ok((handle, _)) => {
                // Always: the background can turn see-through while it is open.
                clear_root_background(handle, cx);
                cx.activate(true);
                let _ = handle.update(cx, |_, window, _| window.activate_window());
                self.settings = Some(handle);
            }
            Err(error) => tracing::error!(%error, "could not open the settings window"),
        }
    }

    /// A solid window (SPEC M6: only search and settings see through).
    pub(super) fn open_onboarding(&mut self, cx: &mut App) {
        let options = WindowOptions {
            titlebar: Some(TitlebarOptions {
                title: Some("Magi".into()),
                ..Default::default()
            }),
            window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                None,
                onboarding::SIZE,
                cx,
            ))),
            window_min_size: Some(onboarding::MIN_SIZE),
            focus: true,
            show: true,
            ..Default::default()
        };
        let (host, live, events) = (self.host.clone(), self.live.clone(), self.events.clone());
        match gpui_kit::open_window(options, cx, move |window, cx| {
            cx.new(|cx| OnboardingView::new(host, live, events, window, cx))
        }) {
            Ok((handle, _)) => {
                cx.activate(true);
                let _ = handle.update(cx, |_, window, _| window.activate_window());
                self.onboarding = Some(handle);
            }
            Err(error) => tracing::error!(%error, "could not open the onboarding window"),
        }
    }

    /// `teach`: opened some other way than the hotkey, so it shows it.
    pub(super) fn open_window(&mut self, teach: bool, cx: &mut App) {
        let opening = Opening {
            at: Instant::now(),
            teach: (teach && self.hotkey.is_some()).then(|| self.ui.hotkey.clone()),
        };
        let backdrop = theme::backdrop(
            self.ui.transparency_mode,
            self.ui.transparency_intensity,
            magi_core::platform::backdrop_support(),
        );
        // Multi-monitor: the display under the cursor, else the primary.
        let display = magi_core::platform::display_under_cursor().map(DisplayId::from);
        let full = Bounds::centered(display, size(view::WIDTH, view::MAX_HEIGHT), cx);
        let options = WindowOptions {
            titlebar: None,
            window_bounds: Some(WindowBounds::Windowed(Bounds {
                origin: full.origin,
                size: size(view::WIDTH, view::OPEN_HEIGHT),
            })),
            display_id: display,
            kind: if magi_core::platform::popup_takes_focus() {
                WindowKind::PopUp
            } else {
                WindowKind::Floating
            },
            is_movable: false,
            is_resizable: false,
            focus: true,
            show: true,
            window_background: theme::window_background(backdrop),
            ..Default::default()
        };
        let (host, live, events) = (self.host.clone(), self.live.clone(), self.events.clone());
        match gpui_kit::open_window(options, cx, move |window, cx| {
            cx.new(|cx| SearchView::new(host, live, events, backdrop, opening, window, cx))
        }) {
            Ok((handle, view)) => {
                // However it closes (hotkey, Esc, opening a file).
                set_search_open(&self.live, true, cx);
                let live = self.live.clone();
                cx.observe_release(&view, move |_, cx| set_search_open(&live, false, cx))
                    .detach();
                if matches!(backdrop, theme::Backdrop::Blurred { .. }) {
                    clear_root_background(handle, cx);
                }
                // Opening only shows the window; Windows' foreground lock can
                // leave another app with the keyboard (e.g. after `--toggle`
                // from a second process). GPUI's activate works around it.
                let _ = handle.update(cx, |_, window, _| window.activate_window());
                self.window = Some(handle)
            }
            Err(error) => tracing::error!(%error, "could not open the search window"),
        }
    }
}

fn set_search_open(live: &Entity<Live>, open: bool, cx: &mut App) {
    live.update(cx, |live, cx| {
        live.search_open = open;
        cx.notify();
    });
}

/// Brings a window forward; false when it is not open (never opened, or
/// closed by the user).
pub(super) fn activate(window: Option<AnyWindowHandle>, cx: &mut App) -> bool {
    window.is_some_and(|w| {
        // An app with no Dock icon (macOS Accessory) is not brought forward by
        // its window: without this, settings opens behind the other apps.
        // A no-op on Windows and Linux.
        cx.activate(true);
        w.update(cx, |_, window, _| window.activate_window())
            .is_ok()
    })
}

/// gpui-component paints the theme background (opaque) on the root, hiding
/// a blurred backdrop; Root's own style is applied last. The view paints its
/// own background instead.
fn clear_root_background(handle: AnyWindowHandle, cx: &mut App) {
    let _ = handle.update(cx, |root, _, cx| {
        if let Ok(root) = root.downcast::<base::Root>() {
            root.update(cx, |root, cx| {
                root.style().background = Some(transparent_black().into());
                cx.notify();
            });
        }
    });
}
