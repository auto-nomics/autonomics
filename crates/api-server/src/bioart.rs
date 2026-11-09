//! NIH catalogue access; only bounded PNG files enter the editable canvas.
use axum::{
    Json,
    extract::{Path, Query},
    http::StatusCode,
};
use serde::Deserialize;
use serde_json::{Value, json};
type Response = Result<Json<Value>, (StatusCode, Json<Value>)>;
fn upstream(error: impl std::fmt::Display) -> (StatusCode, Json<Value>) {
    tracing::warn!(%error,"BioArt request failed");
    (
        StatusCode::BAD_GATEWAY,
        Json(json!({"error":"BioArt upstream unavailable"})),
    )
}
#[derive(Deserialize)]
pub(crate) struct Search {
    q: String,
    #[serde(default)]
    start: u64,
}
pub(crate) async fn search(Query(query): Query<Search>) -> Response {
    if query.q.trim().is_empty() || query.q.len() > 300 {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({"error":"search requires 1..300 characters"})),
        ));
    }
    let client = bioart::BioArtClient::new().map_err(upstream)?;
    let page = client
        .search(query.q.trim(), query.start, 12)
        .await
        .map_err(upstream)?;
    Ok(Json(json!(page)))
}
pub(crate) async fn image(Path(id): Path<u64>) -> Response {
    let client = bioart::BioArtClient::new().map_err(upstream)?;
    let image = client.canvas_image(id).await.map_err(upstream)?;
    Ok(Json(json!(image)))
}
