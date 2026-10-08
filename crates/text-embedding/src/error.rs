//! Errors returned by text embedding providers and vector helpers.

/// Something went wrong turning text into vectors.
#[derive(Debug, thiserror::Error)]
pub enum EmbeddingError {
    #[error("embedding dimension mismatch: {expected} != {actual}")]
    DimensionMismatch { expected: usize, actual: usize },

    #[error("cannot average no vectors")]
    NoVectors,

    /// No vector exists for the requested id.
    #[error("no vector for id {id:?}")]
    MissingVector { id: String },

    /// A provider returned unusable vectors for the given inputs.
    #[error("embedding provider: {0}")]
    Provider(String),
}

pub type Result<T> = std::result::Result<T, EmbeddingError>;
