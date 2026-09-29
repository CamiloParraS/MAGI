#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() -> std::process::ExitCode {
    magi_desktop::run(std::env::args().skip(1).collect())
}
