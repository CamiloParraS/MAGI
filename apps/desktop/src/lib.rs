//! The magi desktop app (SPEC.md M6, ADR-0011): thin GPUI views over
//! `magi_core::host::Host`. No business logic lives here.

pub mod app;
pub mod hotkey;
pub mod i18n;
pub mod instance;
mod logging;
pub mod onboarding;
pub mod search;
pub mod settings;
pub mod theme;
pub mod tray;

use std::process::ExitCode;

use app::AppEvent;

/// The launch at login's argument: start in the tray, open no window.
pub const BACKGROUND_ARG: &str = "--background";

pub fn run(args: Vec<String>) -> ExitCode {
    logging::init();
    let toggle = args.iter().any(|a| a == "--toggle");
    let background = args.iter().any(|a| a == BACKGROUND_ARG);
    let event = |toggle| {
        if toggle {
            AppEvent::Toggle
        } else {
            AppEvent::Show
        }
    };
    let name = instance::socket_name();
    let (tx, rx) = async_channel::unbounded();
    match instance::claim(&name) {
        // Already running: a launch at login has nothing to show.
        Ok(instance::Claim::Secondary) if background => return ExitCode::SUCCESS,
        Ok(instance::Claim::Secondary) => {
            let message = if toggle { "toggle" } else { "show" };
            return match instance::forward(&name, message) {
                Ok(()) => ExitCode::SUCCESS,
                Err(error) => {
                    tracing::error!(%error, "could not reach the running magi");
                    ExitCode::FAILURE
                }
            };
        }
        Ok(instance::Claim::Primary(listener)) => {
            let tx = tx.clone();
            instance::serve(listener, move |message| {
                let _ = tx.send_blocking(event(message == "toggle"));
            });
        }
        Err(error) => tracing::warn!(%error, "running without single-instance"),
    }
    app::run((!background).then(|| event(toggle)), (tx, rx))
}
