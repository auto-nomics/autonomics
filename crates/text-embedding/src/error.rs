//! Errors returned by text embedding providers and vector helpers.

/// Something went wrong turning text into vectors.
#[derive(Debug, thiserror::Error)]
pub enum Error {
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

    /// A Hugging Face Hub operation failed (download, auth, cache).
    /// Only present with the `onnx` feature, which owns the hf-hub
    /// dependency.
    #[cfg(feature = "onnx")]
    #[error("hugging face hub: {0}")]
    Hf(#[from] hf_hub::HFError),

    #[error("unknown error: {0}")]
    Unknown(String),
}

impl From<&str> for Error {
    fn from(value: &str) -> Self {
        Self::Unknown(value.to_string())
    }
}

pub type Result<T> = std::result::Result<T, Error>;
