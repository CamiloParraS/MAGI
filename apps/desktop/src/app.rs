//! The GPUI side of the app. Every outside signal becomes an [`AppEvent`]
//! on one channel, handled on the main thread by [`Shell::handle`].

use std::ops::ControlFlow;
use std::process::ExitCode;
use std::time::Instant;

use gpui_kit::*;
use magi_core::config::UiConfig;
use magi_core::dto::IndexState;
use magi_core::host::{Host, HostEvent, HostPaths};

use crate::i18n::Lang;
use crate::search::view::{self, Live, SearchView};
use crate::settings::view::{self as settings, SettingsView};
use crate::theme;
use crate::tray::{Tray, TrayAction};

pub enum AppEvent {
    /// A plain launch: open the settings window, or bring it forward. The
    /// app stays usable without a tray (SPEC.md §6.3).
    Settings,
    /// Close the search window if it is open, else open it.
    Toggle,
    Host(HostEvent),
    Tray(TrayAction),
    /// The settings window saved new `ui` settings; apply them live.
    Ui(UiConfig),
}

pub type Events = async_channel::Sender<AppEvent>;

/// Starts the engine host and GPUI; returns when the app quits.
pub fn run(first: AppEvent, (tx, rx): (Events, async_channel::Receiver<AppEvent>)) -> ExitCode {
    let host_tx = tx.clone();
    let host = match Host::start(HostPaths::default(), move |event| {
        let _ = host_tx.send_blocking(AppEvent::Host(event));
    }) {
        Ok(host) => host,
        Err(error) => {
            tracing::error!(%error, "the engine host failed to start");
            return ExitCode::FAILURE;
        }
    };
    let ui = host.settings().map(|c| c.ui).unwrap_or_default();
    let _ = tx.send_blocking(first);
    gpui_kit::application()
        .with_assets(gpui_kit::assets::Assets)
        .run(move |cx| {
        gpui_kit::init(cx);
        SearchView::bind_keys(cx);
        // The app lives in the tray; closing the search window never quits.
        cx.set_quit_mode(QuitMode::Explicit);
        let hotkey_tx = tx.clone();
        let hotkey = crate::hotkey::register(&ui.hotkey, move || {
            let _ = hotkey_tx.send_blocking(AppEvent::Toggle);
        })
        .inspect_err(|error| {
            tracing::warn!(%error, hotkey = %ui.hotkey, "hotkey not registered; `magi --toggle` still works")
        })
        .ok();
        let tray_tx = tx.clone();
        let lang = Lang::current(ui.language);
        let live = cx.new(|_| Live::new(lang, ui.theme.clone()));
        let tray = Tray::new(lang, move |action| {
            let _ = tray_tx.send_blocking(AppEvent::Tray(action));
        })
        .inspect_err(|error| tracing::warn!(%error, "no tray icon; use the hotkey or `magi --toggle`"))
        .ok();
        let mut shell = Shell {
            host,
            ui,
            live,
            window: None,
            settings: None,
            events: tx.clone(),
            _hotkey: hotkey,
            tray,
            paused: false,
        };
        cx.spawn(async move |cx| {
            while let Ok(event) = rx.recv().await {
                if cx.update(|cx| shell.handle(event, cx)).is_break() {
                    break;
                }
            }
        })
        .detach();
    });
    ExitCode::SUCCESS
}

struct Shell {
    host: Host,
    ui: UiConfig,
    live: Entity<Live>,
    window: Option<AnyWindowHandle>,
    settings: Option<AnyWindowHandle>,
    /// For windows that report back to the shell (settings).
    events: Events,
    /// Dropping the manager unregisters the hotkey.
    _hotkey: Option<global_hotkey::GlobalHotKeyManager>,
    tray: Option<Tray>,
    paused: bool,
}

impl Shell {
    fn handle(&mut self, event: AppEvent, cx: &mut App) -> ControlFlow<()> {
        match event {
            AppEvent::Toggle => {
                if !self.close_window(cx) {
                    self.open_window(cx);
                }
            }
            AppEvent::Host(HostEvent::Status(status)) => {
                self.paused = status.state == IndexState::Paused;
                if let Some(tray) = &self.tray {
                    tray.show_status(&status);
                }
                self.live.update(cx, |live, cx| {
                    live.status = Some(status);
                    cx.notify();
                });
            }
            AppEvent::Host(HostEvent::Features(features)) => {
                self.live.update(cx, |live, cx| {
                    live.features = features;
                    cx.notify();
                });
            }
            AppEvent::Tray(TrayAction::OpenSearch) => {
                if !activate(self.window, cx) {
                    self.open_window(cx);
                }
            }
            AppEvent::Ui(ui) => {
                let lang = Lang::current(ui.language);
                if let Some(tray) = &mut self.tray {
                    tray.set_lang(lang, self.live.read(cx).status.as_ref());
                }
                self.live.update(cx, |live, cx| {
                    live.lang = lang;
                    live.theme = ui.theme.clone();
                    cx.notify();
                });
                // The search window reads the rest when it next opens.
                self.ui = ui;
            }
            AppEvent::Settings | AppEvent::Tray(TrayAction::OpenSettings) => {
                if !activate(self.settings, cx) {
                    self.open_settings(cx);
                }
            }
            AppEvent::Tray(TrayAction::TogglePause) => {
                let (host, paused) = (self.host.clone(), self.paused);
                cx.background_executor()
                    .spawn(async move {
                        let done = host
                            .engine()
                            .and_then(|e| if paused { e.resume() } else { e.pause() });
                        if let Err(error) = done {
                            tracing::warn!(%error, "pause/resume failed");
                        }
                    })
                    .detach();
            }
            AppEvent::Tray(TrayAction::Quit) => {
                self.close_window(cx);
                // Blocks until the engine stops; quitting during the startup
                // walk waits for it (M6 Plan 2 known gap).
                self.host.shutdown();
                cx.quit();
                return ControlFlow::Break(());
            }
        }
        ControlFlow::Continue(())
    }

    /// GPUI cannot hide a window on Windows (`cx.hide()` is a no-op there),
    /// so the search window is closed and reopened instead (ADR-0011).
    fn close_window(&mut self, cx: &mut App) -> bool {
        self.window
            .take()
            .is_some_and(|w| w.update(cx, |_, window, _| window.remove_window()).is_ok())
    }

    /// A normal, solid window (ADR-0011: only the search window is blurred).
    fn open_settings(&mut self, cx: &mut App) {
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
            focus: true,
            show: true,
            ..Default::default()
        };
        let (host, live, events) = (self.host.clone(), self.live.clone(), self.events.clone());
        match gpui_kit::open_window(options, cx, move |window, cx| {
            cx.new(|cx| SettingsView::new(host, live, events, window, cx))
        }) {
            Ok((handle, _)) => {
                let _ = handle.update(cx, |_, window, _| window.activate_window());
                self.settings = Some(handle);
            }
            Err(error) => tracing::error!(%error, "could not open the settings window"),
        }
    }

    fn open_window(&mut self, cx: &mut App) {
        let opened_at = Instant::now();
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
            kind: WindowKind::PopUp,
            is_movable: false,
            is_resizable: false,
            focus: true,
            show: true,
            window_background: theme::window_background(backdrop),
            ..Default::default()
        };
        let (host, live) = (self.host.clone(), self.live.clone());
        match gpui_kit::open_window(options, cx, move |window, cx| {
            cx.new(|cx| SearchView::new(host, live, backdrop, opened_at, window, cx))
        }) {
            Ok((handle, _)) => {
                // gpui-component paints the theme background (opaque) on the
                // root, hiding the backdrop; Root's own style is applied last.
                if matches!(backdrop, theme::Backdrop::Blurred { .. }) {
                    let _ = handle.update(cx, |root, _, cx| {
                        if let Ok(root) = root.downcast::<base::Root>() {
                            root.update(cx, |root, cx| {
                                root.style().background = Some(transparent_black().into());
                                cx.notify();
                            });
                        }
                    });
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

/// Brings a window forward; false when it is not open (never opened, or
/// closed by the user).
fn activate(window: Option<AnyWindowHandle>, cx: &mut App) -> bool {
    window.is_some_and(|w| {
        w.update(cx, |_, window, _| window.activate_window())
            .is_ok()
    })
}
