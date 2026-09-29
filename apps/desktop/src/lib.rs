//! The magi desktop app (SPEC.md M6, ADR-0011): thin GPUI views over
//! `magi_core::host::Host`. No business logic lives here.

pub mod app;
pub mod hotkey;
pub mod instance;
mod logging;
pub mod search;
pub mod theme;
pub mod tray;

use std::process::ExitCode;

use app::AppEvent;

pub fn run(args: Vec<String>) -> ExitCode {
    logging::init();
    let toggle = args.iter().any(|a| a == "--toggle");
    let name = instance::socket_name();
    let (tx, rx) = async_channel::unbounded();
    match instance::claim(&name) {
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
                let event = if message == "toggle" {
                    AppEvent::Toggle
                } else {
                    AppEvent::Show
                };
                let _ = tx.send_blocking(event);
            });
        }
        Err(error) => tracing::warn!(%error, "running without single-instance"),
    }
    let first = if toggle {
        AppEvent::Toggle
    } else {
        AppEvent::Show
    };
    app::run(first, (tx, rx))
}
