//! Errors returned by the Ensembl SDK.

use thiserror::Error;

/// Errors produced by Ensembl clients and converters.
#[derive(Debug, Error)]
pub enum EnsemblError {
    /// HTTP transport failure.
    #[error("Ensembl HTTP request failed: {0}")]
    Http(#[from] reqwest::Error),

    /// The API returned a non-success status.
    #[error("Ensembl API returned HTTP {status}: {body}")]
    Status { status: u16, body: String },

    /// A JSON response could not be decoded.
    #[error("failed to decode Ensembl response: {0}")]
    Decode(#[from] serde_json::Error),

    /// A required identifier is empty.
    #[error("invalid Ensembl identifier: {0}")]
    InvalidIdentifier(String),

    /// A region is not in an accepted Ensembl form.
    #[error("invalid genomic region {0:?}; expected for example '13:32355000-32357000:1'")]
    InvalidRegion(String),

    /// A batch request must contain at least one identifier.
    #[error("Ensembl batch request cannot be empty")]
    EmptyBatch,

    /// An input value is outside the API or SDK limit.
    #[error("invalid Ensembl request parameter: {0}")]
    InvalidParameter(String),
}

/// Convenient result alias for the SDK.
pub type Result<T> = std::result::Result<T, EnsemblError>;
