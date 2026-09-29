//! The magi desktop app (SPEC.md M6, ADR-0011): thin GPUI views over
//! `magi_core::host::Host`. No business logic lives here.

pub mod search;
pub mod theme;

use std::process::ExitCode;

pub fn run(_args: Vec<String>) -> ExitCode {
    ExitCode::SUCCESS
}
