use thiserror::Error;

/// Errors produced by the Semantic Scholar client.
#[derive(Debug, Error)]
pub enum S2Error {
    #[error("API rate limit reached (retry after {retry_after_seconds:?} seconds)")]
    RateLimited { retry_after_seconds: Option<u64> },

    #[error("Semantic Scholar response exceeds the supported size limit")]
    ResponseTooLarge,

    #[error("invalid Semantic Scholar response: {0}")]
    InvalidResponse(String),

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
