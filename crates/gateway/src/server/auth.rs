//! Bearer authentication for browser-facing HTTP surfaces.

use std::sync::Arc;

use axum::Json;
use axum::extract::{Request, State};
use axum::http::{StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use serde_json::json;

pub(crate) async fn bearer_auth(
    State(expected): State<Arc<String>>,
    request: axum::extract::Request,
    next: Next,
) -> Result<Response, Response> {
    let supplied = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "));
    let valid =
        supplied.is_some_and(|supplied| constant_time_eq(supplied.as_bytes(), expected.as_bytes()));
    if !valid {
        let body = Json(json!({ "error": "missing or invalid bearer token" })).into_response();
        return Err((
            StatusCode::UNAUTHORIZED,
            [(header::WWW_AUTHENTICATE, "Bearer")],
            body,
        )
            .into_response());
    }
    Ok(next.run(request).await)
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.iter()
        .zip(right)
        .fold(0, |difference, (left, right)| difference | (left ^ right))
        == 0
}
