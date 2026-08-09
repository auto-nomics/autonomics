use thiserror::Error;

/// Errors produced by the OpenAlex client.
#[derive(Debug, Error)]
pub enum OpenAlexError {
    #[error("HTTP request failed: {0}")]
    Http(#[from] reqwest::Error),

    #[error("API returned status {status}: {body}")]
    Status { status: u16, body: String },

    #[error("invalid or missing parameter: {0}")]
    Param(String),

    #[error("failed to parse response: {0}")]
    Parse(#[from] serde_json::Error),

    #[error("entity not found: {0}")]
    NotFound(String),
}

/// Result alias for OpenAlex operations.
pub type Result<T> = std::result::Result<T, OpenAlexError>;
