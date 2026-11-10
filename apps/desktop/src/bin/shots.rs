//! `just shots`: photographs every screen of the app in fixed states.

fn main() -> std::process::ExitCode {
    magi_desktop::shots::main(std::env::args().skip(1).collect())
}
