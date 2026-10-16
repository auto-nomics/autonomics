use thiserror::Error;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Error)]
pub enum Error {
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
