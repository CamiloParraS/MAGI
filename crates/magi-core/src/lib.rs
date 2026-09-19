//! magi-core: all business logic for magi. No Tauri dependency — see SPEC.md §5.1.

pub mod chunk;
pub mod config;
pub mod db;
pub mod discovery;
pub mod dto;
pub mod embed;
pub mod engine;
pub mod error;
pub mod extract;
pub mod index;
pub mod ocr;
pub mod paths;
pub mod platform;
pub mod qr;
pub mod search;
pub mod thumbs;
pub mod watch;

pub use engine::{Engine, EngineHandle};
pub use error::{Error, Result};

/// Health check used by the M0 "Hello" Tauri command.
pub fn ping() -> &'static str {
    "pong"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ping_returns_pong() {
        assert_eq!(ping(), "pong");
    }
}
