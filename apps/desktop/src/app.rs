//! The GPUI side of the app. Every outside signal becomes an [`AppEvent`]
//! on one channel, handled on the main thread by [`Shell::handle`].

use std::ops::ControlFlow;
use std::process::ExitCode;
use std::time::Instant;

use gpui_kit::*;
use magi_core::config::{Onboarding, UiConfig};
use magi_core::dto::IndexState;
use magi_core::host::{Host, HostEvent, HostPaths};

use crate::hotkey::{self, Hotkey};
use crate::i18n::Lang;
use crate::onboarding::{self, OnboardingView};
use crate::search::view::{self, Live, SearchView};
use crate::settings::view::{self as settings, SettingsView};
use crate::theme;
use crate::tray::{Tray, TrayAction};

pub enum AppEvent {
    /// A plain launch: onboarding until it is done (FR-10), else the
    /// settings window; or bring either forward. The app stays
    /// usable without a tray (SPEC.md §6.3).
    Show,
    /// Open the settings window, or bring it forward (the search window's
    /// gear).
    Settings,
    /// `magi --onboarding`: onboarding again from its first step (a reset
    /// for trying it; folders and features stay).
    Onboarding,
    /// Close the search window if it is open, else open it.
    Toggle,
    Host(HostEvent),
    Tray(TrayAction),
    /// The settings window saved new `ui` settings; apply them live.
    Ui(UiConfig),
    /// The settings recorder's new hotkey: register it in place of the
    /// current one and reply whether that worked, before it is saved.
    Hotkey(String, async_channel::Sender<bool>),
    /// The tray's Quit, or Settings › General's for when there is no tray
    /// (SPEC.md §6.3).
    Quit,
}

pub type Events = async_channel::Sender<AppEvent>;

/// Asks the shell to register `spec` in place of the hotkey (FR-7
/// conflict detection); true when it did, and the caller then saves it.
pub async fn register_hotkey(events: &Events, spec: String) -> bool {
    let (reply, registered) = async_channel::bounded(1);
    events.send(AppEvent::Hotkey(spec, reply)).await.is_ok() && registered.recv().await == Ok(true)
}

/// Starts the engine host and GPUI; returns when the app quits. `first` is
/// `None` for a launch at login, which opens no window.
pub fn run(
    first: Option<AppEvent>,
    (tx, rx): (Events, async_channel::Receiver<AppEvent>),
) -> ExitCode {
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
    // Re-registered at every start: the command follows a moved or updated
    // executable.
    if ui.launch_at_login {
        set_launch_at_login(true);
    }
    if let Some(first) = first {
        let _ = tx.send_blocking(first);
    }
    gpui_kit::application()
        .with_assets(AppAssets)
        .run(move |cx| {
        gpui_kit::init(cx);
        SearchView::bind_keys(cx);
        onboarding::bind_keys(cx);
        // `with_animation` then renders each animation's static state.
        cx.set_reduce_motion(magi_core::platform::reduce_motion());
        // The app lives in the tray; closing the search window never quits.
        cx.set_quit_mode(QuitMode::Explicit);
        let hotkey_tx = tx.clone();
        let mut hotkey = Hotkey::new(move || {
            let _ = hotkey_tx.send_blocking(AppEvent::Toggle);
        })
        .inspect_err(|error| tracing::warn!(%error, "no hotkey; `magi --toggle` still works"))
        .ok();
        let mut live = Live::new(Lang::current(ui.language), ui.theme.clone());
        if let Some(Err(error)) = hotkey.as_mut().map(|h| h.set(&ui.hotkey)) {
            tracing::warn!(%error, hotkey = %ui.hotkey, "hotkey not registered; `magi --toggle` still works");
            // Settings asks for another shortcut (SPEC.md §6.2).
            live.hotkey_conflict = Some(ui.hotkey.clone());
        }
        let tray_tx = tx.clone();
        let lang = live.lang;
        let live = cx.new(|_| live);
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
            onboarding: None,
            events: tx.clone(),
            hotkey,
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
    onboarding: Option<AnyWindowHandle>,
    /// For windows that report back to the shell (settings).
    events: Events,
    hotkey: Option<Hotkey>,
    tray: Option<Tray>,
    paused: bool,
}

impl Shell {
    fn handle(&mut self, event: AppEvent, cx: &mut App) -> ControlFlow<()> {
        match event {
            AppEvent::Toggle => {
                self.live.update(cx, |live, cx| {
                    live.hotkey_presses += 1;
                    cx.notify();
                });
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
                if ui.launch_at_login != self.ui.launch_at_login {
                    set_launch_at_login(ui.launch_at_login);
                }
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
            AppEvent::Hotkey(spec, reply) => {
                let set = if hotkey::reserved(&spec) {
                    Err("reserved by the system".to_owned())
                } else {
                    self.hotkey
                        .as_mut()
                        .ok_or_else(|| "no hotkey manager".to_owned())
                        .and_then(|h| h.set(&spec))
                };
                if let Err(error) = &set {
                    tracing::warn!(%error, hotkey = %spec, "hotkey change refused");
                }
                self.live.update(cx, |live, cx| {
                    live.hotkey_conflict = set.is_err().then_some(spec);
                    cx.notify();
                });
                let _ = reply.try_send(set.is_ok());
            }
            AppEvent::Show => {
                if activate(self.onboarding, cx) || activate(self.settings, cx) {
                    return ControlFlow::Continue(());
                }
                // Onboarding saves its progress; until it is done it resumes.
                if self.ui.onboarding == Onboarding::Done {
                    self.open_settings(cx);
                } else {
                    self.open_onboarding(cx);
                }
            }
            AppEvent::Onboarding => {
                if let Some(open) = self.onboarding.take() {
                    let _ = open.update(cx, |_, window, _| window.remove_window());
                }
                // The window opens at the saved step.
                match self.host.update_settings(&serde_json::json!({
                    "ui": { "onboarding": Onboarding::Folders }
                })) {
                    Ok(config) => self.ui = config.ui,
                    Err(error) => tracing::warn!(%error, "could not reset the onboarding"),
                }
                self.open_onboarding(cx);
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
            AppEvent::Quit | AppEvent::Tray(TrayAction::Quit) => {
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

    /// A normal window with the search window's background (SPEC M6).
    fn open_settings(&mut self, cx: &mut App) {
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
                let _ = handle.update(cx, |_, window, _| window.activate_window());
                self.settings = Some(handle);
            }
            Err(error) => tracing::error!(%error, "could not open the settings window"),
        }
    }

    /// A solid window (SPEC M6: only search and settings see through).
    fn open_onboarding(&mut self, cx: &mut App) {
        let options = WindowOptions {
            titlebar: Some(TitlebarOptions {
                title: Some(self.live.read(cx).lang.strings().welcome.into()),
                ..Default::default()
            }),
            window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                None,
                onboarding::SIZE,
                cx,
            ))),
            focus: true,
            show: true,
            ..Default::default()
        };
        let (host, live, events) = (self.host.clone(), self.live.clone(), self.events.clone());
        match gpui_kit::open_window(options, cx, move |window, cx| {
            cx.new(|cx| OnboardingView::new(host, live, events, window, cx))
        }) {
            Ok((handle, _)) => {
                let _ = handle.update(cx, |_, window, _| window.activate_window());
                self.onboarding = Some(handle);
            }
            Err(error) => tracing::error!(%error, "could not open the onboarding window"),
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
        let (host, live, events) = (self.host.clone(), self.live.clone(), self.events.clone());
        match gpui_kit::open_window(options, cx, move |window, cx| {
            cx.new(|cx| SearchView::new(host, live, events, backdrop, opened_at, window, cx))
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

// The default bundle embeds only the component icons; these are the
// settings sidebar's and rows' extras.
gpui_kit::assets::icon_assets!(
    ExtraIcons,
    [
        Sparkles,
        ChartColumn,
        Keyboard,
        Trash,
        Power,
        // Keycaps (onboarding).
        ArrowBigUp,
        ChevronUp,
        Command,
        Option,
    ]
);

struct AppAssets;

impl AssetSource for AppAssets {
    fn load(&self, path: &str) -> Result<Option<std::borrow::Cow<'static, [u8]>>> {
        match ExtraIcons.load(path)? {
            Some(bytes) => Ok(Some(bytes)),
            None => gpui_kit::assets::Assets.load(path),
        }
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let mut paths = gpui_kit::assets::Assets.list(path)?;
        paths.extend(ExtraIcons.list(path)?);
        paths.sort();
        paths.dedup();
        Ok(paths)
    }
}

/// Registers or removes the launch at login (`ui.launch_at_login`); the
/// login launch passes `--background`, which opens no window. Per user,
/// never system-wide.
fn set_launch_at_login(on: bool) {
    let done = std::env::current_exe()
        .map_err(anyhow::Error::from)
        .and_then(|exe| {
            let launch = auto_launch::AutoLaunchBuilder::new()
                .set_app_name("Magi")
                .set_app_path(&exe.to_string_lossy())
                .set_args(&[crate::BACKGROUND_ARG])
                .set_windows_enable_mode(auto_launch::WindowsEnableMode::CurrentUser)
                .build()?;
            if on {
                launch.enable()
            } else {
                launch.disable()
            }
            .map_err(anyhow::Error::from)
        });
    if let Err(error) = done {
        tracing::warn!(%error, on, "could not change the launch at login");
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
fn activate(window: Option<AnyWindowHandle>, cx: &mut App) -> bool {
    window.is_some_and(|w| {
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
