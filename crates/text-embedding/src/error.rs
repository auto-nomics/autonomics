//! Errors returned by text embedding providers and vector helpers.

use crate::service;

/// Something went wrong turning text into vectors.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A provider returned unusable vectors for the given inputs.
    #[error("embedding provider: {0}")]
    Provider(String),

    /// The local model service (install/open) failed.
    #[error("embedding service error: {0}")]
    Service(#[from] service::error::Error),

    #[error("unknown error: {0}")]
    Unknown(String),
}

impl From<&str> for Error {
    fn from(value: &str) -> Self {
        Self::Unknown(value.to_string())
    }
}

pub type Result<T> = std::result::Result<T, Error>;
