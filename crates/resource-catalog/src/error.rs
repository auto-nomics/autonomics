//! Error type for the resource catalog.

use thiserror::Error;

/// Errors surfaced by the resource catalog.
#[derive(Debug, Error)]
pub enum ResourceError {
    #[error("resource '{0}' is already registered with a different address")]
    Duplicate(String),
    #[error("unknown resource '{0}'")]
    UnknownResource(String),
    #[error("invalid resource: {0}")]
    Validation(String),
    #[error("resource kind mismatch: expected {expected}, got {found} for '{name}'")]
    KindMismatch {
        name: String,
        expected: &'static str,
        found: &'static str,
    },
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("persistence error: {0}")]
    Persistence(String),
    #[error("global resource catalog already initialized")]
    GlobalAlreadySet,
    #[error("serialization error: {0}")]
    Serde(#[from] serde_json::Error),
    #[error("storage error: {0}")]
    Storage(String),
    #[error("backend '{0}' is not registered")]
    UnknownBackend(String),
}

/// Convenience alias for catalog results.
pub type Result<T> = std::result::Result<T, ResourceError>;
