//! Error types for the `hdl` crate.

use thiserror::Error;

/// Errors produced by the HDL-L pipeline.
#[derive(Debug, Error)]
pub enum HdlError {
    #[error("HDL-L input error: {0}")]
    Input(String),
    #[error("HDL-L numeric error: {0}")]
    Numeric(String),
    #[error("HDL-L optimisation did not converge: {0}")]
    NoConverge(String),
    #[error("HDL-L reference error: {0}")]
    Reference(String),
    #[error("LAVA primitive failed: {0}")]
    Lava(#[from] lava::LavaError),
}

pub type Result<T> = std::result::Result<T, HdlError>;

impl From<HdlError> for String {
    fn from(e: HdlError) -> String {
        e.to_string()
    }
}
