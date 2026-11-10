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

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PartsEdit {
    element_id: String,
    expected_revision: i64,
    sha256: String,
    edits: Vec<bioicons::parts::Edit>,
}
pub(crate) async fn parts(
    State(shared): State<Shared>,
    Path((project, id)): Path<(String, String)>,
    Query(input): Query<Palette>,
) -> Response {
    work(shared, move |store| {
        visualization_assets::parts(
            store,
            &project,
            &id,
            input.expected_revision,
            &input.element_id,
        )
    })
    .await
}
pub(crate) async fn edit_parts(
    State(shared): State<Shared>,
    Path((project, id)): Path<(String, String)>,
    Json(input): Json<PartsEdit>,
) -> Response {
    work(shared, move |store| {
        Ok(json!(visualization_assets::edit_parts(
            store,
            &project,
            &id,
            input.expected_revision,
            &input.element_id,
            &input.sha256,
            &input.edits
        )?))
    })
    .await
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct GeometryQuery {
    element_id: String,
    expected_revision: i64,
    sha256: String,
    part_id: String,
}
pub(crate) async fn geometry(
    State(shared): State<Shared>,
    Path((project, id)): Path<(String, String)>,
    Query(input): Query<GeometryQuery>,
) -> Response {
    work(shared, move |store| {
        visualization_assets::geometry(
            store,
            &project,
            &id,
            input.expected_revision,
            &input.element_id,
            &input.sha256,
            &input.part_id,
        )
    })
    .await
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Inset {
    expected_revision: i64,
    inset: visualization::inset::Spec,
}
pub(crate) async fn image_info(
    State(shared): State<Shared>,
    Path((project, id)): Path<(String, String)>,
    Query(input): Query<Palette>,
) -> Response {
    work(shared, move |store| {
        visualization_assets::image_info(
            store,
            &project,
            &id,
            input.expected_revision,
            &input.element_id,
        )
    })
    .await
}
pub(crate) async fn inset(
    State(shared): State<Shared>,
    Path((project, id)): Path<(String, String)>,
    Json(input): Json<Inset>,
) -> Response {
    work(shared, move |store| {
        Ok(json!(visualization_assets::inset(
            store,
            &project,
            &id,
            input.expected_revision,
            &input.inset
        )?))
    })
    .await
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RefreshInset {
    element_id: String,
    expected_revision: i64,
    #[serde(default)]
    discard_detail_edits: bool,
}
pub(crate) async fn inset_status(
    State(shared): State<Shared>,
    Path((project, id)): Path<(String, String)>,
    Query(input): Query<Palette>,
) -> Response {
    work(shared, move |store| {
        visualization_assets::inset_status(
            store,
            &project,
            &id,
            input.expected_revision,
            &input.element_id,
        )
    })
    .await
}
pub(crate) async fn refresh_inset(
    State(shared): State<Shared>,
    Path((project, id)): Path<(String, String)>,
    Json(input): Json<RefreshInset>,
) -> Response {
    work(shared, move |store| {
        Ok(json!(visualization_assets::refresh_inset(
            store,
            &project,
            &id,
            input.expected_revision,
            &input.element_id,
            input.discard_detail_edits
        )?))
    })
    .await
}
