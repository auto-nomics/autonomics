use thiserror::Error;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Error)]
pub enum Error {
    #[error("unknown error: {0}")]
    Unknown(String),
    #[error("{0}")]
    Turso(#[from] turso::Error),
    #[error("serialization error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("not found: {0}")]
    NotFound(String),
}

impl From<&str> for Error {
    fn from(value: &str) -> Self {
        Self::Unknown(value.into())
    }
}
