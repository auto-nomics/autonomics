use serde_json::Value;

/// Errors returned by the Open Targets client.
#[derive(Debug, thiserror::Error)]
pub enum OpenTargetsError {
    /// An HTTP transport error (DNS, connection, timeout, …).
    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),

    /// The server returned a non-2xx status code.
    #[error("HTTP {status}: {body}")]
    Status { status: u16, body: String },

    /// The GraphQL endpoint returned an `errors` array.
    #[error("graphql errors: {0}")]
    GraphQl(String),

    /// The response body could not be deserialized into the requested type.
    #[error("deserialize error: {0}")]
    Deserialize(#[from] serde_json::Error),

    /// The top-level data key was missing entirely.
    #[error("missing \"data\" key in response: {0}")]
    EmptyData(Value),
}

pub type Result<T> = std::result::Result<T, OpenTargetsError>;
