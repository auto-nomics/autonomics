use thiserror::Error;

#[derive(Debug, Error)]
pub enum ScpError {
    #[error("HTTP request failed: {0}")]
    Http(#[from] reqwest::Error),

    #[error("Single Cell Portal returned {status}: {body}")]
    Status { status: u16, body: String },

    #[error("invalid or missing parameter: {0}")]
    Param(String),

    #[error("failed to parse Single Cell Portal response: {0}")]
    Parse(#[from] serde_json::Error),
}

pub type Result<T> = std::result::Result<T, ScpError>;
