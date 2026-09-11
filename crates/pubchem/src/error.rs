use thiserror::Error;

#[derive(Debug, Error)]
pub enum PubChemError {
    #[error("HTTP request failed: {0}")]
    Http(#[from] reqwest::Error),

    #[error("PubChem API returned HTTP {status}: {message}")]
    Api { status: u16, message: String },

    #[error("failed to parse PubChem response: {0}")]
    Parse(#[from] serde_json::Error),

    #[error("invalid compound identifier: {0}")]
    InvalidIdentifier(String),

    #[error("unsupported identifier type: {0}")]
    UnsupportedIdentifierType(String),

    #[error("URL error: {0}")]
    Url(#[from] url::ParseError),
}

pub type Result<T> = std::result::Result<T, PubChemError>;
