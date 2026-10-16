use thiserror::Error;

#[derive(Debug, Error)]
pub enum HcaError {
    #[error("HTTP request failed: {0}")]
    Http(#[from] reqwest::Error),

    #[error("HCA API returned {status}: {body}")]
    Status { status: u16, body: String },

    #[error("invalid or missing parameter: {0}")]
    Param(String),

    #[error("record not found: {0}")]
    NotFound(String),

    #[error("failed to parse HCA response: {0}")]
    Parse(#[from] serde_json::Error),
}

pub type Result<T> = std::result::Result<T, HcaError>;
