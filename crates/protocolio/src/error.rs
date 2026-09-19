use thiserror::Error;

/// Errors returned by the protocols.io SDK.
#[derive(Debug, Error)]
pub enum ProtocolioError {
    /// `PROTOCOLS_IO_ACCESS_TOKEN` was not set and no token was supplied.
    #[error("protocols.io access token is required; set PROTOCOLS_IO_ACCESS_TOKEN")]
    MissingToken,

    /// HTTP transport failure.
    #[error("protocols.io HTTP request failed: {0}")]
    Http(#[from] reqwest::Error),

    /// The service returned a non-success HTTP status.
    #[error("protocols.io API returned HTTP {status}: {body}")]
    Status { status: u16, body: String },

    /// The service returned HTTP 429.
    #[error("protocols.io rate limit exceeded (retry after {:?})", retry_after)]
    RateLimited { retry_after: Option<u64> },

    /// A successful HTTP response carried a non-zero API status code.
    #[error("protocols.io API error {status_code}: {status_text}")]
    Api {
        status_code: u32,
        status_text: String,
    },

    /// A JSON response could not be decoded.
    #[error("failed to decode protocols.io response: {0}")]
    Decode(#[from] serde_json::Error),

    /// A virtual-file-system write failed.
    #[error("failed to write protocols.io output: {0}")]
    Storage(String),

    /// A request argument violates an API or SDK constraint.
    #[error("invalid protocols.io request parameter: {0}")]
    InvalidParameter(String),

    /// A response omitted a required resource.
    #[error("protocols.io response omitted {field}")]
    MissingField { field: String },
}

/// Convenient result alias for the SDK.
pub type Result<T> = std::result::Result<T, ProtocolioError>;
