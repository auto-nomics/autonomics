use thiserror::Error;

use agentik_types::errors::AnthropicError;

use crate::store::error::Error as StoreError;

pub type Result<T> = std::result::Result<T, Error>;

/// Errors raised while extracting and consolidating durable memory.
#[derive(Debug, Error)]
pub enum Error {
    #[error("{operation}: {source}")]
    Source {
        operation: String,
        source: Box<crate::Error>,
    },

    #[error("{operation}: {source}")]
    Store {
        operation: String,
        source: StoreError,
    },

    #[error("serialization error: {0}")]
    Serialization(String),

    #[error("{operation}: {source}")]
    ModelRequest {
        operation: String,
        source: AnthropicError,
    },

    #[error("{operation}: {message}")]
    ModelOutput { operation: String, message: String },
}

impl Error {
    pub(super) fn source(operation: impl Into<String>, source: crate::Error) -> Self {
        Self::Source {
            operation: operation.into(),
            source: Box::new(source),
        }
    }

    pub(super) fn store(operation: impl Into<String>, source: StoreError) -> Self {
        Self::Store {
            operation: operation.into(),
            source,
        }
    }

    pub(super) fn serialization(message: serde_json::Error) -> Self {
        Self::Serialization(message.to_string())
    }

    pub(super) fn model_request(operation: impl Into<String>, source: AnthropicError) -> Self {
        Self::ModelRequest {
            operation: operation.into(),
            source,
        }
    }

    pub(super) fn model_output(operation: impl Into<String>, message: impl Into<String>) -> Self {
        Self::ModelOutput {
            operation: operation.into(),
            message: message.into(),
        }
    }
}

impl From<StoreError> for Error {
    fn from(source: StoreError) -> Self {
        Self::Store {
            operation: "knowledge pipeline queue".to_string(),
            source,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wraps_source_context() {
        let error = Error::source(
            "load session",
            crate::Error::Source("db unavailable".into()),
        );
        assert_eq!(
            error.to_string(),
            "load session: memory source error: db unavailable"
        );
    }

    #[test]
    fn wraps_store_context() {
        let error = Error::store(
            "claim memory phase 2",
            StoreError::Other("lease held".into()),
        );
        assert_eq!(error.to_string(), "claim memory phase 2: lease held");
    }
}
