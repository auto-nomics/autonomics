//! Error types for the `cpassoc` crate.

use thiserror::Error;

/// Errors produced by the CPASSOC pipeline.
#[derive(Debug, Error)]
pub enum CpassocError {
    #[error("CPASSOC input error: {0}")]
    Input(String),
    #[error("CPASSOC numeric error: {0}")]
    Numeric(String),
}

pub type Result<T> = std::result::Result<T, CpassocError>;

impl From<CpassocError> for String {
    fn from(e: CpassocError) -> String {
        e.to_string()
    }
}
