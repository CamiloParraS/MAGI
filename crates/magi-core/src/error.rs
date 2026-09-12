//! Crate-wide error type. Variants are added as each subsystem lands.

use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("io error at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("database error: {0}")]
    Db(#[from] rusqlite::Error),

    #[error("invalid config: {0}")]
    ConfigParse(#[from] toml::de::Error),

    #[error("could not serialize config: {0}")]
    ConfigSerialize(#[from] toml::ser::Error),

    #[error("invalid exclude glob {glob:?}: {reason}")]
    InvalidGlob { glob: String, reason: String },

    #[error("root path does not exist: {}", .0.display())]
    RootNotFound(PathBuf),

    #[error("root {} is nested under existing root {}", .path.display(), .conflicts_with.display())]
    NestedRoot {
        path: PathBuf,
        conflicts_with: PathBuf,
    },

    #[error("root {} is already registered", .0.display())]
    RootAlreadyExists(PathBuf),

    #[error("root id {0} not found")]
    RootIdNotFound(i64),
}

pub type Result<T> = std::result::Result<T, Error>;
