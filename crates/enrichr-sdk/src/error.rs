//! Errors produced by the Enrichr SDK.

use thiserror::Error;

/// Errors produced by Enrichr and Speedrichr clients.
#[derive(Debug, Error)]
pub enum EnrichrError {
    /// The HTTP transport failed before Enrichr returned a response.
    #[error("Enrichr HTTP request failed: {0}")]
    Http(#[from] reqwest::Error),

    /// Enrichr returned a non-success HTTP response. The body is usually an
    /// HTML error page rather than JSON.
    #[error("Enrichr API returned HTTP {status}: {body}")]
    Status { status: u16, body: String },

    /// Enrichr returned HTTP 200 with an empty or invalid result envelope,
    /// for example an enrichment request against an unknown library.
    #[error("Enrichr API error: {0}")]
    Api(String),

    /// A JSON payload did not match the SDK model.
    #[error("failed to decode Enrichr response: {0}")]
    Decode(#[from] serde_json::Error),

    /// A request was empty or otherwise invalid before it was sent.
    #[error("invalid Enrichr request: {0}")]
    InvalidRequest(String),
}

/// Convenient result alias for the SDK.
pub type Result<T> = std::result::Result<T, EnrichrError>;
