use thiserror::Error;

#[derive(Debug, Error)]
pub enum TischError {
    #[error("HTTP request failed: {0}")]
    Http(#[from] reqwest::Error),

    #[error("TISCH returned {status}: {body}")]
    Status { status: u16, body: String },

    #[error("invalid or missing parameter: {0}")]
    Param(String),

    #[error("failed to parse TISCH page: {0}")]
    Parse(String),
}

pub type Result<T> = std::result::Result<T, TischError>;
