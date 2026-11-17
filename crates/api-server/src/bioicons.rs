//! BioIcons catalogue and normalized vector assets; independent per-icon licences.
use axum::{
    Json,
    extract::{Path, Query},
    http::StatusCode,
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::LazyLock;
static CLIENT: LazyLock<bioicons::Result<bioicons::BioIconsClient>> =
    LazyLock::new(bioicons::BioIconsClient::new);
type Response = Result<Json<Value>, (StatusCode, Json<Value>)>;
fn error(error: impl std::fmt::Display) -> (StatusCode, Json<Value>) {
    tracing::warn!(%error,"BioIcons request failed");
    (
        StatusCode::BAD_GATEWAY,
        Json(json!({"error":"BioIcons asset unavailable or unsupported SVG"})),
    )
}
#[derive(Deserialize)]
pub(crate) struct Search {
    q: String,
    author: Option<String>,
    #[serde(default)]
    start: usize,
}
pub(crate) async fn search(Query(input): Query<Search>) -> Response {
    if input.q.trim().is_empty()
        || input.q.len() > 300
        || input
            .author
            .as_deref()
            .is_some_and(|a| a.trim().is_empty() || a.len() > 300)
    {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(
                json!({"error":"search requires 1..300 query bytes and an optional nonblank author of at most 300 bytes"}),
            ),
        ));
    }
    let client = CLIENT.as_ref().map_err(error)?;
    Ok(Json(json!(
        client
            .search_with_author(&input.q, input.start, 12, input.author.as_deref())
            .await
            .map_err(error)?
    )))
}
pub(crate) async fn vector(Path(id): Path<String>) -> Response {
    if id.len() != 64 || !id.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({"error":"invalid asset id"})),
        ));
    }
    let client = CLIENT.as_ref().map_err(error)?;
    Ok(Json(json!(client.vector(&id).await.map_err(error)?)))
}
