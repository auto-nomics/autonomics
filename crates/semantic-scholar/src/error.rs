use thiserror::Error;

/// Errors produced by the Semantic Scholar client.
#[derive(Debug, Error)]
pub enum S2Error {
    #[error("HTTP request failed: {0}")]
    Http(#[from] reqwest::Error),

    #[error("API returned status {status}: {body}")]
    Status { status: u16, body: String },

    #[error("invalid or missing parameter: {0}")]
    Param(String),

    #[error("failed to parse response: {0}")]
    Parse(#[from] serde_json::Error),
}

/// Result alias for Semantic Scholar operations.
pub type Result<T> = std::result::Result<T, S2Error>;
