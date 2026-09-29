//! The GPUI side of the app. Every outside signal becomes an [`AppEvent`]
//! on one channel, handled on the main thread by [`Shell::handle`].

use std::ops::ControlFlow;
use std::process::ExitCode;
use std::time::Instant;

use gpui_kit::*;
use magi_core::config::UiConfig;
use magi_core::host::{Host, HostEvent, HostPaths};

use crate::theme::{self, Backdrop};

pub enum AppEvent {
    /// Open the search window, or bring it forward.
    Show,
    /// Close the search window if it is open, else open it.
    Toggle,
    Host(HostEvent),
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
    gpui_kit::application().run(move |cx| {
        gpui_kit::init(cx);
        // The app lives in the tray; closing the search window never quits.
        cx.set_quit_mode(QuitMode::Explicit);
        let mut shell = Shell {
            host,
            ui,
            window: None,
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
    #[expect(dead_code, reason = "the search window takes it in Task 5")]
    host: Host,
    ui: UiConfig,
    window: Option<AnyWindowHandle>,
}

impl Shell {
    fn handle(&mut self, event: AppEvent, cx: &mut App) -> ControlFlow<()> {
        match event {
            AppEvent::Show => {
                if !self.activate_window(cx) {
                    self.open_window(cx);
                }
            }
            AppEvent::Toggle => {
                if !self.close_window(cx) {
                    self.open_window(cx);
                }
            }
            AppEvent::Host(_) => {}
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

    fn activate_window(&self, cx: &mut App) -> bool {
        self.window.is_some_and(|w| {
            w.update(cx, |_, window, _| window.activate_window())
                .is_ok()
        })
    }

    fn open_window(&mut self, cx: &mut App) {
        let opened_at = Instant::now();
        let backdrop = theme::backdrop(
            self.ui.transparency_mode,
            self.ui.transparency_intensity,
            magi_core::platform::backdrop_support(),
        );
        let options = WindowOptions {
            titlebar: None,
            window_bounds: Some(WindowBounds::centered(size(px(680.), px(460.)), cx)),
            kind: WindowKind::PopUp,
            is_movable: false,
            is_resizable: false,
            focus: true,
            show: true,
            window_background: theme::window_background(backdrop),
            ..Default::default()
        };
        match gpui_kit::open_window(options, cx, move |_, cx| {
            cx.new(|_| Placeholder {
                backdrop,
                opened_at,
            })
        }) {
            Ok((handle, _)) => self.window = Some(handle),
            Err(error) => tracing::error!(%error, "could not open the search window"),
        }
    }
}

/// Stands in for the search window until Task 5.
struct Placeholder {
    backdrop: Backdrop,
    opened_at: Instant,
}

impl Render for Placeholder {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        tracing::info!(elapsed = ?self.opened_at.elapsed(), "search window frame");
        let alpha = match self.backdrop {
            Backdrop::Mica { tint_alpha } => tint_alpha,
            Backdrop::Solid => 1.0,
        };
        div()
            .size_full()
            .bg(hsla(0., 0., 0.97, alpha))
            .child("magi")
    }
}
