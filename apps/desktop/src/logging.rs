//! Logs go to `<data_dir>/logs/magi.log`: release builds have no console
//! (`windows_subsystem`), so a startup failure must land somewhere readable.

use std::fs::{self, OpenOptions};
use std::path::PathBuf;
use std::sync::Mutex;

use tracing_subscriber::EnvFilter;

pub(crate) fn path() -> PathBuf {
    magi_core::paths::data_dir().join("logs").join("magi.log")
}

pub(crate) fn init() {
    let path = path();
    let file = path
        .parent()
        .map_or(Ok(()), fs::create_dir_all)
        .and_then(|()| OpenOptions::new().create(true).append(true).open(&path));
    let filter = EnvFilter::try_from_env("MAGI_LOG").unwrap_or_else(|_| EnvFilter::new("info"));
    let builder = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_ansi(false);
    match file {
        Ok(file) => builder.with_writer(Mutex::new(file)).init(),
        Err(_) => builder.with_writer(std::io::stderr).init(),
    }
}
