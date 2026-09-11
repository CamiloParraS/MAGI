//! Crate-wide error type. Variants are added as each subsystem lands.

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("not yet implemented")]
    Unimplemented,
}

pub type Result<T> = std::result::Result<T, Error>;
