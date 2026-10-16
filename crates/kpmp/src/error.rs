use thiserror::Error;

#[derive(Debug, Error)]
pub enum KpmpError {
    #[error("HTTP request failed: {0}")]
    Http(#[from] reqwest::Error),

    #[error("KPMP API returned {status}: {message}")]
    Status { status: u16, message: String },

    #[error("GraphQL error: {0}")]
    Graphql(String),

    #[error("invalid or missing parameter: {0}")]
    Param(String),

    #[error("failed to parse KPMP response: {0}")]
    Parse(#[from] serde_json::Error),
}

pub type Result<T> = std::result::Result<T, KpmpError>;
