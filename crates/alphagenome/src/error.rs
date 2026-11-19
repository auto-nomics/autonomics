use thiserror::Error;

/// Errors returned by the AlphaGenome SDK.
#[derive(Debug, Error)]
pub enum Error {
    /// Transport / RPC failure, with the gRPC status details.
    #[error("alphagenome RPC failed: {0}")]
    Grpc(#[from] tonic::Status),

    /// A tensor payload could not be decoded (unsupported dtype, truncated
    /// chunk stream, shape mismatch).
    #[error("tensor decode error: {0}")]
    Decode(String),

    /// A variant / interval string could not be parsed.
    #[error("parse error: {0}")]
    Parse(String),

    /// No API key configured.
    #[error("missing API key: set the {0} environment variable")]
    MissingApiKey(String),

    /// The API key was rejected.
    #[error("authentication failed (check your API key)")]
    Unauthenticated,
}

impl Error {
    /// Maps a gRPC status to a user-friendly error where possible,
    /// mirroring the Python SDK's error mapping.
    pub(crate) fn from_status(status: tonic::Status) -> Self {
        match status.code() {
            tonic::Code::Unauthenticated => Error::Unauthenticated,
            _ => Error::Grpc(status),
        }
    }
}

pub type Result<T> = std::result::Result<T, Error>;
