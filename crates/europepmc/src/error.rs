use thiserror::Error;

/// Errors produced by the Europe PMC client.
#[derive(Debug, Error)]
pub enum EuropePmcError {
    #[error("HTTP request failed: {0}")]
    Http(#[from] reqwest::Error),

    #[error("API returned non-OK status: {status} – {body}")]
    Status { status: u16, body: String },

    #[error("invalid or missing parameter: {0}")]
    Param(String),

    #[error("failed to parse response: {0}")]
    Parse(#[from] serde_json::Error),
}

/// Result alias for Europe PMC operations.
pub type Result<T> = std::result::Result<T, EuropePmcError>;
