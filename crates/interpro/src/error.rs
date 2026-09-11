use thiserror::Error;

#[derive(Debug, Error)]
pub enum InterProError {
    #[error("HTTP request failed: {0}")]
    Http(#[from] reqwest::Error),

    #[error("InterPro API returned HTTP {status}: {message}")]
    Api { status: u16, message: String },

    #[error("failed to parse InterPro response: {0}")]
    Parse(#[from] serde_json::Error),

    #[error("invalid InterPro accession {0:?}; expected IPR followed by six digits")]
    InvalidAccession(String),
}

pub type Result<T> = std::result::Result<T, InterProError>;
