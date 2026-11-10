//! The GPUI side of the app. Every outside signal becomes an [`AppEvent`]
//! on one channel, handled on the main thread by [`Shell::handle`].

mod assets;
mod start_failure;
mod windows;

use std::ops::ControlFlow;
use std::process::ExitCode;

use gpui_kit::*;
use magi_core::config::{Onboarding, UiConfig};
use magi_core::dto::PauseReason;
use magi_core::host::{Host, HostEvent, HostPaths};

use crate::ask;
use crate::ask::view::AskView;
use crate::hotkey::{self, Hotkey};
use crate::i18n::Lang;
use crate::live::Live;
use crate::onboarding;
use crate::search::view::SearchView;
use crate::theme;
use crate::tray::{Tray, TrayAction};
use crate::ui::SCAN_EXIT;

use assets::AppAssets;
use start_failure::show_start_failure;
use windows::activate;
pub use windows::{AskSlot, show_ask};

pub enum AppEvent {
    /// A plain launch: onboarding until it is done (FR-10), else search,
    /// which then shows the shortcut; or bring either forward. Opening the
    /// app again is how someone who forgot the shortcut gets back, and
    /// keeps it usable without a tray (SPEC.md §6.3).
    Show,
    /// Open the settings window, or bring it forward (the search window's
    /// gear).
    Settings,
    /// Ask about what is on screen: open the Ask window, or give the open
    /// one these contents.
    Ask(ask::Request),
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
            // Release builds have no console: say so in a window.
            show_start_failure(error.to_string());
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
        // Menu-bar only on macOS (SPEC.md §6.1); after GPUI's own launch setup.
        magi_core::platform::hide_dock_icon();
        cx.set_window_appearance(theme::chrome(&ui.theme));
        SearchView::bind_keys(cx);
        AskView::bind_keys(cx);
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
        // From the config alone: no keychain read and no request.
        live.account = host.account_state().unwrap_or_default();
        live.ask_enabled = ui.ask_magi;
        // Wayland may refuse the hotkey manager outright; settings then says
        // to bind `magi --toggle` in the desktop (SPEC.md §6.3).
        if hotkey.is_none() && magi_core::platform::is_wayland() {
            live.hotkey_conflict = Some(ui.hotkey.clone());
        }
        if let Some(Err(error)) = hotkey.as_mut().map(|h| h.set(&ui.hotkey)) {
            tracing::warn!(%error, hotkey = %ui.hotkey, "hotkey not registered; `magi --toggle` still works");
            // Settings asks for another shortcut (SPEC.md §6.2).
            live.hotkey_conflict = Some(ui.hotkey.clone());
        }
        let tray_tx = tx.clone();
        let lang = live.lang;
        let registered = hotkey.is_some() && live.hotkey_conflict.is_none();
        let live = cx.new(|_| live);
        let mut tray = Tray::new(lang, move |action| {
            let _ = tray_tx.send_blocking(AppEvent::Tray(action));
        })
        .inspect_err(|error| tracing::warn!(%error, "no tray icon; use the hotkey or `magi --toggle`"))
        .ok();
        if let Some(tray) = &mut tray {
            tray.set_hotkey(registered.then_some(ui.hotkey.as_str()));
        }
        let mut shell = Shell {
            host,
            ui,
            live,
            window: None,
            settings: None,
            onboarding: None,
            ask: None,
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
    ask: AskSlot,
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
                    self.open_window(false, cx);
                }
            }
            AppEvent::Host(HostEvent::Status(status)) => {
                self.paused = status.paused_for == Some(PauseReason::User);
                if let Some(tray) = &self.tray {
                    tray.show_status(&status);
                }
                let ended = self.live.update(cx, |live, cx| {
                    cx.notify();
                    live.set_status(status)
                });
                // The scan row has left; stop drawing it.
                if ended {
                    let live = self.live.clone();
                    cx.spawn(async move |cx| {
                        cx.background_executor().timer(SCAN_EXIT).await;
                        live.update(cx, |live, cx| {
                            live.scan_left = None;
                            cx.notify();
                        });
                    })
                    .detach();
                }
            }
            AppEvent::Host(HostEvent::Features(features)) => {
                self.live.update(cx, |live, cx| {
                    live.features = features;
                    cx.notify();
                });
            }
            AppEvent::Host(HostEvent::Permissions(issues)) => {
                self.live.update(cx, |live, cx| {
                    live.permissions = issues;
                    cx.notify();
                });
            }
            AppEvent::Tray(TrayAction::OpenSearch) => {
                if !activate(self.window, cx) {
                    self.open_window(true, cx);
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
                cx.set_window_appearance(theme::chrome(&ui.theme));
                self.live.update(cx, |live, cx| {
                    live.lang = lang;
                    live.theme = ui.theme.clone();
                    live.ask_enabled = ui.ask_magi;
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
                } else if let Some(tray) = &mut self.tray {
                    tray.set_hotkey(Some(&spec));
                }
                self.live.update(cx, |live, cx| {
                    live.hotkey_conflict = set.is_err().then_some(spec);
                    cx.notify();
                });
                let _ = reply.try_send(set.is_ok());
            }
            AppEvent::Show => {
                if activate(self.onboarding, cx) {
                    return ControlFlow::Continue(());
                }
                // Onboarding saves its progress; until it is done it resumes.
                if self.ui.onboarding != Onboarding::Done {
                    self.open_onboarding(cx);
                } else if !activate(self.window, cx) {
                    self.open_window(true, cx);
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
            AppEvent::Ask(request) => {
                let backdrop = self.backdrop();
                show_ask(&mut self.ask, &self.host, &self.live, backdrop, request, cx);
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
}

/// Registers or removes the launch at login (`ui.launch_at_login`); the
/// login launch passes `--background`, which opens no window. Per user,
/// never system-wide.
fn set_launch_at_login(on: bool) {
    let done = magi_core::paths::installed_exe()
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
