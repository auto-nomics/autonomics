use thiserror::Error;

#[derive(Debug, Error)]
pub enum NemoError {
    #[error("HTTP request failed: {0}")]
    Http(#[from] reqwest::Error),

    #[error("NeMO Analytics returned {status}: {body}")]
    Status { status: u16, body: String },

    #[error("NeMO Analytics rejected the query: {0}")]
    Api(String),

    #[error("invalid or missing parameter: {0}")]
    Param(String),

    #[error("failed to parse NeMO Analytics response: {0}")]
    Parse(#[from] serde_json::Error),
}

pub type Result<T> = std::result::Result<T, NemoError>;
