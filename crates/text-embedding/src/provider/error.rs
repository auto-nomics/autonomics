use thiserror::Error;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Error)]
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

    #[error("{0}")]
    Unknown(String),
}

impl From<&str> for Error {
    fn from(value: &str) -> Self {
        Self::Unknown(value.to_string())
    }
}
