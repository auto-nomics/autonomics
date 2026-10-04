//! Error-to-response mapping for gateway handlers.

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::json;

/// Errors that map directly onto HTTP responses.
pub enum GatewayError {
    Message(String),
    Status(StatusCode, String),
}

impl From<String> for GatewayError {
    fn from(message: String) -> Self {
        Self::Message(message)
    }
}

impl IntoResponse for GatewayError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            GatewayError::Message(message) => (StatusCode::BAD_REQUEST, message),
            GatewayError::Status(status, message) => (status, message),
        };
        (status, Json(json!({ "error": message }))).into_response()
    }
}

pub(crate) type GatewayResult<T> = std::result::Result<T, GatewayError>;
