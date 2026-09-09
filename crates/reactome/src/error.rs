//! Errors returned by the Reactome SDK.

use thiserror::Error;

/// Errors produced by Reactome clients and converters.
#[derive(Debug, Error)]
pub enum ReactomeError {
    /// HTTP transport failure.
    #[error("Reactome HTTP request failed: {0}")]
    Http(#[from] reqwest::Error),

    /// The API returned a non-success status.
    #[error("Reactome API returned HTTP {status}: {body}")]
    Status { status: u16, body: String },

    /// A JSON response could not be decoded.
    #[error("failed to decode Reactome response: {0}")]
    Decode(#[from] serde_json::Error),

    /// A required identifier is empty.
    #[error("invalid Reactome identifier: {0}")]
    InvalidIdentifier(String),

    /// An input value is outside the API or SDK limit.
    #[error("invalid Reactome request parameter: {0}")]
    InvalidParameter(String),

    /// An analysis token is empty or malformed.
    #[error("invalid Reactome analysis token: {0}")]
    InvalidToken(String),
}

/// Convenient result alias for the SDK.
pub type Result<T> = std::result::Result<T, ReactomeError>;
