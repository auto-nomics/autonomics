use thiserror::Error;

/// Errors returned by the RCSB PDB SDK.
#[derive(Debug, Error)]
pub enum RcsbError {
    #[error("HTTP request failed: {0}")]
    Http(#[from] reqwest::Error),

    #[error("RCSB API returned HTTP {status}: {message}")]
    Api { status: u16, message: String },

    #[error("invalid RCSB entry ID {0:?}; expected four ASCII alphanumeric characters")]
    InvalidEntryId(String),

    #[error("invalid or missing parameter: {0}")]
    Param(String),

    #[error("failed to parse RCSB response: {0}")]
    Parse(#[from] serde_json::Error),

    #[error("binary format {0} cannot be decoded as UTF-8 text")]
    BinaryText(&'static str),
}

/// Result alias for RCSB PDB operations.
pub type Result<T> = std::result::Result<T, RcsbError>;

pub(crate) fn api_error(status: reqwest::StatusCode, body: &str) -> RcsbError {
    let message = serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|value| {
            value
                .get("message")
                .or_else(|| value.get("error"))
                .and_then(|v| v.as_str())
                .map(str::to_owned)
        })
        .unwrap_or_else(|| {
            let trimmed = body.trim();
            if trimmed.is_empty() {
                status.to_string()
            } else {
                trimmed.to_owned()
            }
        });

    RcsbError::Api {
        status: status.as_u16(),
        message,
    }
}
