use thiserror::Error;

/// Errors produced by the EasyScholar client.
#[derive(Debug, Error)]
pub enum EasyScholarError {
    #[error("HTTP request failed: {0}")]
    Http(#[from] reqwest::Error),

    #[error("API returned non-OK status: {status} – {body}")]
    Status { status: u16, body: String },

    #[error("failed to parse response: {0}")]
    Parse(#[from] serde_json::Error),
}

/// Result alias for EasyScholar operations.
pub type Result<T> = std::result::Result<T, EasyScholarError>;
