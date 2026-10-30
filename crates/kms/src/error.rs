use thiserror::Error;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Error)]
pub enum Error {
    /// Catch-all for business-rule violations raised by [`crate::service::KmsService`].
    /// The message is surfaced verbatim to callers (and to the agent-facing
    /// tool layer), so it must read as a sentence, not as a Rust diagnostic.
    #[error("{0}")]
    Unknown(String),
    /// Storage-layer failure. Displayed through the underlying
    /// [`crate::storage::error::StorageError`] message.
    #[error("{0}")]
    Storage(#[from] crate::storage::error::StorageError),
}

impl From<&str> for Error {
    fn from(value: &str) -> Self {
        Self::Unknown(value.to_string())
    }
}

impl From<String> for Error {
    fn from(value: String) -> Self {
        Self::Unknown(value)
    }
}

impl From<uuid::Error> for Error {
    fn from(value: uuid::Error) -> Self {
        Self::Storage(crate::storage::error::StorageError::from(value))
    }
}

impl From<turso::Error> for Error {
    fn from(value: turso::Error) -> Self {
        Self::Storage(crate::storage::error::StorageError::from(value))
    }
}
