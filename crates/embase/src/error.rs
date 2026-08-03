use thiserror::Error;

/// Errors produced by the Embase client.
#[derive(Debug, Error)]
pub enum EmbaseError {
    #[error("HTTP request failed: {0}")]
    Http(#[from] reqwest::Error),

    #[error("API returned non-OK status: {status} – {body}")]
    Status { status: u16, body: String },

    #[error("invalid or missing parameter: {0}")]
    Param(String),

    #[error("failed to parse response: {0}")]
    Parse(#[from] serde_json::Error),

    #[error("API key is required: set EMBASE_API_KEY or pass it to EmbaseClient::new")]
    MissingApiKey,
}

/// Result alias for Embase operations.
pub type Result<T> = std::result::Result<T, EmbaseError>;
