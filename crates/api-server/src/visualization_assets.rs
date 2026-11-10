//! Palette and colour proposals share the same operation layer as agent tools.
use crate::visualization::Shared;
use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::BTreeMap;
type Response = Result<Json<Value>, (StatusCode, Json<Value>)>;
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Palette {
    element_id: String,
    expected_revision: i64,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Colors {
    element_id: String,
    expected_revision: i64,
    colors: BTreeMap<String, String>,
}
fn error(error: visualization_assets::Error) -> (StatusCode, Json<Value>) {
    let status = match &error {
        visualization_assets::Error::Store(visualization::Error::Conflict) => StatusCode::CONFLICT,
        visualization_assets::Error::Store(visualization::Error::NotFound) => StatusCode::NOT_FOUND,
        visualization_assets::Error::Store(
            visualization::Error::Database(_) | visualization::Error::Json(_),
        ) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error":"visualization storage unavailable"})),
            );
        }
        _ => StatusCode::BAD_REQUEST,
    };
    (status, Json(json!({"error":error.to_string()})))
}
async fn work(
    shared: Shared,
    f: impl FnOnce(&visualization::Store) -> visualization_assets::Result<Value> + Send + 'static,
) -> Response {
    tokio::task::spawn_blocking(move || {
        let service = shared.lock().map_err(|_| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error":"visualization storage unavailable"})),
            )
        })?;
        f(service.store()).map(Json).map_err(error)
    })
    .await
    .map_err(|_| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error":"asset operation failed"})),
        )
    })?
}
pub(crate) async fn palette(
    State(shared): State<Shared>,
    Path((project, id)): Path<(String, String)>,
    Query(input): Query<Palette>,
) -> Response {
    work(shared, move |store| {
        visualization_assets::palette(
            store,
            &project,
            &id,
            input.expected_revision,
            &input.element_id,
        )
    })
    .await
}
pub(crate) async fn recolor(
    State(shared): State<Shared>,
    Path((project, id)): Path<(String, String)>,
    Json(input): Json<Colors>,
) -> Response {
    work(shared, move |store| {
        Ok(json!(visualization_assets::recolor(
            store,
            &project,
            &id,
            input.expected_revision,
            &input.element_id,
            &input.colors
        )?))
    })
    .await
}
