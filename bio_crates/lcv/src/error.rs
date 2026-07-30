//! Error types for the `lcv` crate.

use thiserror::Error;

/// Errors produced by the LCV pipeline.
#[derive(Debug, Error)]
pub enum LcvError {
    #[error("LCV input error: {0}")]
    Input(String),
    #[error("LCV numeric error: {0}")]
    Numeric(String),
}

pub type Result<T> = std::result::Result<T, LcvError>;

impl From<LcvError> for String {
    fn from(e: LcvError) -> String {
        e.to_string()
    }
}
