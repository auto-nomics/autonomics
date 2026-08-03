use thiserror::Error;

/// Errors produced by the arXiv API client.
#[derive(Debug, Error)]
pub enum ArxivError {
    #[error("HTTP request failed: {0}")]
    Http(#[from] reqwest::Error),

    #[error("API returned non-OK status: {status} – {body}")]
    Status { status: u16, body: String },

    #[error("invalid or missing parameter: {0}")]
    Param(String),

    #[error("arXiv API error: {0}")]
    Api(String),

    #[error("failed to parse XML response: {0}")]
    Xml(String),
}

/// Result alias for arXiv operations.
pub type Result<T> = std::result::Result<T, ArxivError>;
