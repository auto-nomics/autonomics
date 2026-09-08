use thiserror::Error;

/// Errors produced by the UniProt client.
#[derive(Debug, Error)]
pub enum UniProtError {
    #[error("HTTP request failed: {0}")]
    Http(#[from] reqwest::Error),

    #[error("API returned non-OK status: {status} – {body}")]
    Status { status: u16, body: String },

    #[error("invalid or missing parameter: {0}")]
    Param(String),

    #[error("failed to parse response: {0}")]
    Parse(#[from] serde_json::Error),

    #[error("timed out after {0}s waiting for a submitted ID mapping job")]
    Timeout(u64),
}

/// Result alias for UniProt operations.
pub type Result<T> = std::result::Result<T, UniProtError>;
