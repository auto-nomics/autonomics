use thiserror::Error;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Error)]
pub enum Error {
    /// A Hugging Face Hub operation failed (download, auth, cache).
    #[error("hugging face hub: {0}")]
    Hf(#[from] hf_hub::HFError),

    #[error("embedding provider error: {0}")]
    Provider(#[from] crate::provider::error::Error),

    #[error("{0}")]
    Unknown(String),
}

impl From<&str> for Error {
    fn from(value: &str) -> Self {
        Self::Unknown(value.to_string())
    }
}
