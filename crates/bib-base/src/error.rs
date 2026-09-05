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
    #[error("MinerU parse failed: {0}")]
    Mineru(#[from] mineru::Error),
    #[error("file storage error: {0}")]
    Storage(#[from] vfs::opendal::Error),
}

impl From<&str> for Error {
    fn from(value: &str) -> Self {
        Self::Unknown(value.into())
    }
}
