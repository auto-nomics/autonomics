use thiserror::Error;

#[derive(Debug, Error)]
pub enum AlphaFoldError {
    #[error("HTTP request failed: {0}")]
    Http(#[from] reqwest::Error),

    #[error("AlphaFold API returned HTTP {status}: {message}")]
    Api { status: u16, message: String },

    #[error("failed to parse AlphaFold response: {0}")]
    Parse(#[from] serde_json::Error),

    #[error("invalid UniProt accession {0:?}; expected 6 or 10 alphanumeric characters")]
    InvalidAccession(String),
}

pub type Result<T> = std::result::Result<T, AlphaFoldError>;
