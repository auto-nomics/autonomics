//! Error type for the LAVA port.

use thiserror::Error;

/// A LAVA computation error (file IO, dimension/format mismatches, numerical
/// failure, out-of-bounds parameters).
#[derive(Debug, Error)]
pub enum LavaError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("input error: {0}")]
    Input(String),

    #[error("dimension mismatch: {0}")]
    Dim(String),

    #[error("numerical error: {0}")]
    Numeric(String),

    #[error("locus {locus}: {msg}")]
    Locus { locus: String, msg: String },

    #[error("{0}")]
    Other(String),
}

impl From<&str> for LavaError {
    fn from(s: &str) -> Self {
        LavaError::Other(s.to_string())
    }
}

impl From<String> for LavaError {
    fn from(s: String) -> Self {
        LavaError::Other(s)
    }
}

pub type Result<T> = std::result::Result<T, LavaError>;
