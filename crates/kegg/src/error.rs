//! Errors returned by the KEGG SDK.

use thiserror::Error;

/// Errors produced by the KEGG REST client, parsers, and agent tools.
#[derive(Debug, Error)]
pub enum KeggError {
    /// HTTP transport failure.
    #[error("KEGG HTTP request failed: {0}")]
    Http(#[from] reqwest::Error),

    /// The REST service returned a non-success status.
    #[error("KEGG API returned HTTP {status}: {body}")]
    Status { status: u16, body: String },

    /// A JSON response, normally BRITE output, could not be decoded.
    #[error("failed to decode KEGG response: {0}")]
    Decode(#[from] serde_json::Error),

    /// A URL could not be constructed from the configured endpoint.
    #[error("invalid KEGG request URL: {0}")]
    Url(#[from] url::ParseError),

    /// An argument violates a KEGG API or SDK constraint.
    #[error("invalid KEGG request parameter: {0}")]
    InvalidParameter(String),
}

/// Convenient result alias for the SDK.
pub type Result<T> = std::result::Result<T, KeggError>;
