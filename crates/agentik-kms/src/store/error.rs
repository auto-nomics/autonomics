use thiserror::Error;

pub type Result<T> = std::result::Result<T, Error>;

/// Errors raised by a durable memory repository implementation.
#[derive(Debug, Error)]
pub enum Error {
    #[error("storage error: {0}")]
    Storage(String),

    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("{0}")]
    Other(String),
}
