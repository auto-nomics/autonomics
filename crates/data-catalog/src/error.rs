use thiserror::Error;

pub type Result<T> = std::result::Result<T, Error>;

/// Errors raised while building, publishing, loading, or querying a catalog.
#[derive(Debug, Error)]
pub enum Error {
    #[error("{0}")]
    Other(String),
}

impl From<String> for Error {
    fn from(message: String) -> Self {
        Self::Other(message)
    }
}

impl From<&str> for Error {
    fn from(message: &str) -> Self {
        Self::Other(message.to_string())
    }
}

/// Bridges typed catalog errors to the service trait's String error type.
impl From<Error> for String {
    fn from(error: Error) -> Self {
        error.to_string()
    }
}
