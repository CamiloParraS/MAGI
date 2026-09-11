//! Engine: owns threads, channels, and lifecycle.
//!
//! Stub for M0. M1 adds config/DB wiring; later milestones add discovery,
//! extraction, embedding, watching, and search (see SPEC.md §5.3).

/// The core engine that Tauri commands and the CLI talk to.
pub struct Engine;

/// Cheap, cloneable handle to a running [`Engine`] (channels + a read-only
/// DB connection pool for search).
pub struct EngineHandle;

impl Engine {
    pub fn ping(&self) -> &'static str {
        crate::ping()
    }
}
