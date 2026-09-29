//! The magi desktop app (SPEC.md M6, ADR-0011): thin GPUI views over
//! `magi_core::host::Host`. No business logic lives here.

pub mod app;
mod logging;
pub mod search;
pub mod theme;

use std::process::ExitCode;

use app::AppEvent;

pub fn run(args: Vec<String>) -> ExitCode {
    logging::init();
    let first = if args.iter().any(|a| a == "--toggle") {
        AppEvent::Toggle
    } else {
        AppEvent::Show
    };
    app::run(first, async_channel::unbounded())
}
