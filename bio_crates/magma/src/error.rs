//! Error type for the MAGMA port.

use thiserror::Error;

/// A MAGMA computation error (file IO, dimension/format mismatches, numerical
/// failure, out-of-bounds parameters).
#[derive(Debug, Error)]
pub enum MagmaError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("input error: {0}")]
    Input(String),

    #[error("dimension mismatch: {0}")]
    Dim(String),

    #[error("numerical error: {0}")]
    Numeric(String),

    #[error("configuration error: {0}")]
    Config(String),

    #[error("{0}")]
    Other(String),
}

impl From<&str> for MagmaError {
    fn from(s: &str) -> Self {
        MagmaError::Other(s.to_string())
    }
}

impl From<String> for MagmaError {
    fn from(s: String) -> Self {
        MagmaError::Other(s)
    }
}

pub type Result<T> = std::result::Result<T, MagmaError>;
