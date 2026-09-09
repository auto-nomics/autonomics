use thiserror::Error;

/// Errors produced by the STRING SDK.
#[derive(Debug, Error)]
pub enum StringError {
    /// The HTTP transport failed before STRING returned a response.
    #[error("STRING HTTP request failed: {0}")]
    Http(#[from] reqwest::Error),

    /// STRING returned a non-success HTTP response.
    #[error("STRING API returned HTTP {status}: {body}")]
    Status { status: u16, body: String },

    /// STRING returned HTTP 200 with a JSON error envelope.
    #[error("STRING API error: {0}")]
    Api(String),

    /// A JSON payload did not match the SDK model.
    #[error("failed to decode STRING response: {0}")]
    Decode(#[from] serde_json::Error),

    /// A request was empty or otherwise invalid before it was sent.
    #[error("invalid STRING request: {0}")]
    InvalidRequest(String),
}

/// Convenient result alias for the SDK.
pub type Result<T> = std::result::Result<T, StringError>;
