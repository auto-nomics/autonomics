use thiserror::Error;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Error)]
pub enum Error {
    #[error("{0}")]
    Turso(#[from] turso::Error),

    #[error("serialization error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("not found: {0}")]
    NotFound(String),

    #[error("edit error: {0}")]
    Edit(String),

    #[error("version conflict: expected {expected}, found {found}")]
    VersionConflict { expected: u32, found: u32 },

    #[error("{0}")]
    Other(String),
}

impl From<&str> for Error {
    fn from(value: &str) -> Self {
        Self::Other(value.into())
    }
}

impl From<String> for Error {
    fn from(value: String) -> Self {
        Self::Other(value)
    }
}
