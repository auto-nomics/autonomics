use thiserror::Error;

/// Errors produced by the OpenGWAS client.
#[derive(Debug, Error)]
pub enum OpengwasError {
    #[error("HTTP request failed: {0}")]
    Http(#[from] reqwest::Error),

    #[error("SQLite error: {0}")]
    Sqlite(#[from] rusqlite::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("URL encoding error: {0}")]
    UrlEncode(#[from] serde_urlencoded::ser::Error),

    #[error("task join error: {0}")]
    TaskJoin(#[from] tokio::task::JoinError),

    #[error("invalid parameter: {0}")]
    Param(String),

    #[error("invalid or missing auth token: {0}")]
    InvalidToken(String),

    #[error("unexpected response: {0}")]
    UnexpectedResponse(String),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("storage error: {0}")]
    Storage(#[from] vfs::opendal::Error),
}

/// Result alias for OpenGWAS operations.
pub type Result<T> = std::result::Result<T, OpengwasError>;
