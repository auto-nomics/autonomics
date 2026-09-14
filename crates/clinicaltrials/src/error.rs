use thiserror::Error;

#[derive(Debug, Error)]
pub enum ClinicalTrialsError {
    #[error("HTTP request failed: {0}")]
    Http(#[from] reqwest::Error),

    #[error("ClinicalTrials.gov API returned HTTP {status}: {message}")]
    Api { status: u16, message: String },

    #[error("failed to parse ClinicalTrials.gov response: {0}")]
    Parse(#[from] serde_json::Error),

    #[error("invalid NCT identifier {0:?}; expected NCT followed by digits")]
    InvalidNctId(String),
}

pub type Result<T> = std::result::Result<T, ClinicalTrialsError>;
