use thiserror::Error;

/// Errors produced by the bioRxiv/medRxiv API client.
#[derive(Debug, Error)]
pub enum BiorxivError {
    #[error("HTTP request failed: {0}")]
    Http(#[from] reqwest::Error),

    #[error("API returned non-OK status: {status} – {body}")]
    Status { status: u16, body: String },

    #[error("invalid or missing parameter: {0}")]
    Param(String),

    #[error("API error: {0}")]
    Api(String),

    #[error("failed to parse JSON response: {0}")]
    Json(#[from] serde_json::Error),
}

/// Result alias for bioRxiv/medRxiv operations.
pub type Result<T> = std::result::Result<T, BiorxivError>;
