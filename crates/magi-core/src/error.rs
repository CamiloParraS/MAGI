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

    #[error("unknown file type {name:?} in indexing.file_types (expected one of {expected})")]
    UnknownFileType { name: String, expected: String },

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

    #[error("extraction of {} timed out after {seconds}s", .path.display())]
    ExtractionTimeout { path: PathBuf, seconds: u64 },

    #[error("extraction of {} panicked: {message}", .path.display())]
    ExtractionPanicked { path: PathBuf, message: String },

    #[error("PDF error: {0}")]
    Pdf(String),

    #[error("HEIC error: {0}")]
    Heic(String),

    #[error("image is {megapixels} MP, over the {limit} MP limit")]
    ImageTooLarge { megapixels: u32, limit: u32 },

    #[error("Office document error: {0}")]
    Office(String),

    #[error("code parse error: {0}")]
    Code(String),

    #[error("invalid model manifest: {0}")]
    ManifestParse(String),

    #[error("model error: {0}")]
    Model(String),
}

pub type Result<T> = std::result::Result<T, Error>;
