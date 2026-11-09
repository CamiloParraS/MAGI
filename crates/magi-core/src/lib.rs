//! magi-core: all business logic for magi. No UI dependency — see SPEC.md §5.1.

pub mod ask;
pub mod chunk;
pub mod config;
pub mod db;
pub mod discovery;
pub mod dto;
pub mod embed;
pub mod engine;
pub mod error;
pub mod extract;
pub mod features;
pub mod host;
pub mod index;
pub mod ocr;
mod onnx;
pub mod paths;
pub mod platform;
pub mod qr;
pub mod search;
pub mod thumbs;
pub mod watch;

pub use engine::{Engine, EngineHandle};
pub use error::{Error, Result};
