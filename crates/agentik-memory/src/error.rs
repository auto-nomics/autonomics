use thiserror::Error;

use crate::store::error::Error as StoreError;

pub type Result<T> = std::result::Result<T, Error>;

/// Errors raised by the memory crate and presented to its consumers.
#[derive(Debug, Error)]
pub enum Error {
    #[error("memory store error: {0}")]
    Storage(#[from] StoreError),

    #[error("memory source error: {0}")]
    Source(String),

    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("{0}")]
    Other(String),
}
