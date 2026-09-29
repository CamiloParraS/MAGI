//! Logs go to `<data_dir>/logs/magi.log`: release builds have no console
//! (`windows_subsystem`), so a startup failure must land somewhere readable.

use std::fs::{self, OpenOptions};
use std::sync::Mutex;

use tracing_subscriber::EnvFilter;

pub(crate) fn init() {
    let dir = magi_core::paths::data_dir().join("logs");
    let file = fs::create_dir_all(&dir).and_then(|()| {
        OpenOptions::new()
            .create(true)
            .append(true)
            .open(dir.join("magi.log"))
    });
    let filter = EnvFilter::try_from_env("MAGI_LOG").unwrap_or_else(|_| EnvFilter::new("info"));
    let builder = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_ansi(false);
    match file {
        Ok(file) => builder.with_writer(Mutex::new(file)).init(),
        Err(_) => builder.with_writer(std::io::stderr).init(),
    }
}
