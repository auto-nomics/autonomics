use std::io;

use thiserror::Error;

pub type Result<T> = std::result::Result<T, DownloadError>;

#[derive(Debug, Error)]
pub enum DownloadError {
    #[error("invalid download source `{resource}`: {message}")]
    InvalidSource { resource: String, message: String },

    #[error("destination `{path}` is invalid: {message}")]
    InvalidDestination { path: String, message: String },

    #[error("destination `{path}` already exists")]
    DestinationExists { path: String },

    #[error("download from `{resource}` failed: {message}")]
    Request { resource: String, message: String },

    #[error("download policy denied `{resource}`: {message}")]
    Denied { resource: String, message: String },

    #[error("resource at `{resource}` is too large ({actual} bytes, limit {limit} bytes)")]
    TooLarge {
        resource: String,
        actual: u64,
        limit: u64,
    },

    #[error("integrity check failed for `{resource}`: {message}")]
    Integrity { resource: String, message: String },

    #[error("cannot write destination `{path}`: {message}")]
    Write { path: String, message: String },

    #[error(transparent)]
    Io(#[from] io::Error),

    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

impl DownloadError {
    pub fn request(resource: impl Into<String>, message: impl Into<String>) -> Self {
        Self::Request {
            resource: resource.into(),
            message: message.into(),
        }
    }

    pub fn denied(resource: impl Into<String>, message: impl Into<String>) -> Self {
        Self::Denied {
            resource: resource.into(),
            message: message.into(),
        }
    }
}

#[cfg(feature = "agent-tools")]
impl From<DownloadError> for agentik_core::tools::ToolError {
    fn from(error: DownloadError) -> Self {
        agentik_core::tools::ToolError::ExecutionFailed {
            source: Box::new(error),
        }
    }
}
