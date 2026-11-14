//! Project-scoped figure REST surface; mount behind the host's authentication layer.
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Path, Query, State},
    http::StatusCode,
    routing::{get, post},
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use visualization::{
    Store,
    service::{ServiceError, VisualizationService},
};

pub type Shared = Arc<Mutex<VisualizationService>>;
type ApiError = (StatusCode, Json<Value>);
type Response = Result<Json<Value>, ApiError>;
fn internal() -> ApiError {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({"error":"visualization storage unavailable"})),
    )
}
fn error(error: ServiceError) -> ApiError {
    let status = match &error {
        ServiceError::Store(visualization::Error::NotFound) => StatusCode::NOT_FOUND,
        ServiceError::Store(visualization::Error::Conflict) => StatusCode::CONFLICT,
        ServiceError::Store(visualization::Error::Invalid(_))
        | ServiceError::Scene(_)
        | ServiceError::Arrangement(_) => StatusCode::BAD_REQUEST,
        _ => return internal(),
    };
    (status, Json(json!({"error":error.to_string()})))
}
async fn work<F>(shared: Shared, action: F) -> Response
where
    F: FnOnce(&mut VisualizationService) -> Result<Value, ServiceError> + Send + 'static,
{
    tokio::task::spawn_blocking(move || {
        let mut service = shared.lock().map_err(|_| internal())?;
        action(&mut service).map(Json).map_err(error)
    })
    .await
    .map_err(|_| internal())?
}
pub fn router(store: Store) -> Router {
    Router::new()
        .route(
            "/projects/{project}/figures/{id}/inset-status",
            get(crate::visualization_assets::inset_status),
        )
        .route(
            "/projects/{project}/figures/{id}/insets/refresh",
            post(crate::visualization_assets::refresh_inset),
        )
        .route(
            "/projects/{project}/figures/{id}/image-info",
            get(crate::visualization_assets::image_info),
        )
        .route(
            "/projects/{project}/figures/{id}/insets",
            post(crate::visualization_assets::inset),
        )
        .route(
            "/projects/{project}/figures/{id}/vector-geometry",
            get(crate::visualization_assets::geometry),
        )
        .route(
            "/projects/{project}/figures/{id}/vector-parts",
            get(crate::visualization_assets::parts).post(crate::visualization_assets::edit_parts),
        )
        .route(
            "/projects/{project}/figures/{id}/vector-palette",
            get(crate::visualization_assets::palette),
        )
        .route(
            "/projects/{project}/figures/{id}/vector-colors",
            post(crate::visualization_assets::recolor),
        )
        .route("/bioicons/search", get(crate::bioicons::search))
        .route("/bioicons/{id}/vector", get(crate::bioicons::vector))
        .route("/bioart/search", get(crate::bioart::search))
        .route("/bioart/{id}/image", get(crate::bioart::image))
        .route("/projects", get(projects).post(create_project))
        .route(
            "/projects/{project}/figures",
            get(figures).post(create_figure),
        )
        .route(
            "/projects/{project}/figures/{id}",
            get(current).put(save).delete(remove),
        )
        .route(
            "/projects/{project}/figures/{id}/revisions/{revision}",
            get(revision),
        )
        .route("/projects/{project}/figures/{id}/operations", post(apply))
        .route("/projects/{project}/figures/{id}/head", get(head))
        .route("/projects/{project}/figures/{id}/revisions", get(revisions))
        .route(
            "/projects/{project}/figures/{id}/revisions/{revision}/preview",
            get(preview).put(save_preview),
        )
        .route(
            "/projects/{project}/figures/{id}/annotations",
            get(annotations).post(annotate),
        )
        .route(
            "/projects/{project}/figures/{id}/annotations/{annotation}/resolve",
            post(resolve_annotation),
        )
        .route(
            "/projects/{project}/figures/{id}/markups",
            get(markups).post(create_markup),
        )
        .route(
            "/projects/{project}/figures/{id}/markups/{markup}",
            get(markup),
        )
        .route(
            "/projects/{project}/figures/{id}/markups/{markup}/resolve",
            post(resolve_markup),
        )
        .layer(DefaultBodyLimit::max(32 * 1024 * 1024))
        .with_state(Arc::new(Mutex::new(VisualizationService::new(store))))
}
#[derive(Deserialize)]
struct ProjectInput {
    id: String,
    title: String,
}
async fn projects(State(s): State<Shared>) -> Response {
    work(s, |service| Ok(json!(service.store().projects()?))).await
}
async fn create_project(State(s): State<Shared>, Json(input): Json<ProjectInput>) -> Response {
    work(s, move |service| {
        Ok(json!(
            service.store().create_project(&input.id, &input.title)?
        ))
    })
    .await
}
#[derive(Deserialize)]
struct CreateInput {
    title: String,
    format: String,
    document: Value,
}
async fn figures(State(s): State<Shared>, Path(project): Path<String>) -> Response {
    work(s, move |service| Ok(json!(service.store().list(&project)?))).await
}
async fn create_figure(
    State(s): State<Shared>,
    Path(project): Path<String>,
    Json(input): Json<CreateInput>,
) -> Response {
    work(s, move |service| {
        Ok(json!(service.store_mut().create(
            &project,
            &input.title,
            &input.format,
            &input.document
        )?))
    })
    .await
}
async fn current(State(s): State<Shared>, Path((project, id)): Path<(String, String)>) -> Response {
    work(s, move |service| {
        let metadata = service.store().get(&project, &id)?;
        let scene = service.store().revision(&project, &id, metadata.revision)?;
        Ok(json!({"metadata":metadata,"scene":scene,"annotations":service.store().annotations(&project,&id)?}))
    }).await
}
#[derive(Deserialize)]
struct SaveInput {
    expected_revision: i64,
    title: String,
    document: Value,
}
async fn save(
    State(s): State<Shared>,
    Path((project, id)): Path<(String, String)>,
    Json(input): Json<SaveInput>,
) -> Response {
    work(s, move |service| {
        Ok(json!(service.store_mut().save(
            &project,
            &id,
            input.expected_revision,
            &input.title,
            &input.document
        )?))
    })
    .await
}
#[derive(Deserialize)]
struct DeleteInput {
    expected_revision: i64,
}
async fn remove(
    State(s): State<Shared>,
    Path((project, id)): Path<(String, String)>,
    Json(input): Json<DeleteInput>,
) -> Response {
    work(s, move |service| {
        service
            .store_mut()
            .delete(&project, &id, input.expected_revision)?;
        Ok(json!({"deleted":true}))
    })
    .await
}
async fn revision(
    State(s): State<Shared>,
    Path((project, id, revision)): Path<(String, String, i64)>,
) -> Response {
    work(s, move |service| {
        Ok(json!(service.store().revision(&project, &id, revision)?))
    })
    .await
}
#[derive(Deserialize)]
struct OperationsInput {
    expected_revision: i64,
    operations: Vec<visualization::scene::Operation>,
}
async fn apply(
    State(s): State<Shared>,
    Path((project, id)): Path<(String, String)>,
    Json(input): Json<OperationsInput>,
) -> Response {
    if input.operations.len() > 1000 {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({"error":"at most 1000 operations per batch"})),
        ));
    }
    work(s, move |service| {
        Ok(json!(service.apply(
            &project,
            &id,
            input.expected_revision,
            &input.operations
        )?))
    })
    .await
}

#[derive(Deserialize)]
struct AnnotationInput {
    expected_revision: i64,
    element_ids: Vec<String>,
    note: String,
}
async fn annotations(
    State(s): State<Shared>,
    Path((project, id)): Path<(String, String)>,
) -> Response {
    work(s, move |service| {
        Ok(json!(service.store().annotations(&project, &id)?))
    })
    .await
}
async fn annotate(
    State(s): State<Shared>,
    Path((project, id)): Path<(String, String)>,
    Json(input): Json<AnnotationInput>,
) -> Response {
    work(s, move |service| {
        Ok(json!(service.store().annotate(
            &project,
            &id,
            input.expected_revision,
            &input.element_ids,
            &input.note
        )?))
    })
    .await
}
async fn resolve_annotation(
    State(s): State<Shared>,
    Path((project, id, annotation)): Path<(String, String, String)>,
) -> Response {
    work(s, move |service| {
        service
            .store()
            .resolve_annotation(&project, &id, &annotation)?;
        Ok(json!({"resolved":true}))
    })
    .await
}

#[derive(Deserialize)]
struct PreviewInput {
    #[serde(rename = "dataURL")]
    data_url: String,
}
async fn preview(
    State(s): State<Shared>,
    Path((project, id, revision)): Path<(String, String, i64)>,
) -> Response {
    work(s, move |service| {
        Ok(json!(service.store().preview(&project, &id, revision)?))
    })
    .await
}
async fn save_preview(
    State(s): State<Shared>,
    Path((project, id, revision)): Path<(String, String, i64)>,
    Json(input): Json<PreviewInput>,
) -> Response {
    work(s, move |service| {
        service
            .store()
            .save_preview(&project, &id, revision, &input.data_url)?;
        Ok(json!({"revision":revision}))
    })
    .await
}

#[derive(Deserialize)]
struct RevisionQuery {
    before: Option<i64>,
    limit: Option<u32>,
}
async fn revisions(
    State(s): State<Shared>,
    Path((project, id)): Path<(String, String)>,
    Query(query): Query<RevisionQuery>,
) -> Response {
    work(s, move |service| {
        Ok(json!(service.store().revisions(
            &project,
            &id,
            query.before,
            query.limit.unwrap_or(20)
        )?))
    })
    .await
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MarkupInput {
    expected_revision: i64,
    note: String,
    strokes: Vec<visualization::markup::Stroke>,
    #[serde(rename = "dataURL")]
    data_url: String,
}
#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct MarkupQuery {
    #[serde(default)]
    page: bool,
    before: Option<String>,
    status: Option<String>,
    limit: Option<u32>,
}
async fn markups(
    State(s): State<Shared>,
    Path((project, id)): Path<(String, String)>,
    Query(query): Query<MarkupQuery>,
) -> Response {
    work(s, move |service| {
        let page = service.store().markups_page(
            &project,
            &id,
            query.before.as_deref(),
            query.status.as_deref(),
            query.limit.unwrap_or(50),
        )?;
        Ok(if query.page {
            json!(page)
        } else {
            json!(page.items)
        })
    })
    .await
}
async fn markup(
    State(s): State<Shared>,
    Path((project, id, markup)): Path<(String, String, String)>,
) -> Response {
    work(s, move |service| {
        Ok(json!(service.store().markup(&project, &id, &markup)?))
    })
    .await
}
async fn create_markup(
    State(s): State<Shared>,
    Path((project, id)): Path<(String, String)>,
    Json(input): Json<MarkupInput>,
) -> Response {
    work(s, move |service| {
        Ok(json!(service.store().create_markup(
            &project,
            &id,
            input.expected_revision,
            &input.note,
            &input.strokes,
            &input.data_url
        )?))
    })
    .await
}
async fn resolve_markup(
    State(s): State<Shared>,
    Path((project, id, markup)): Path<(String, String, String)>,
) -> Response {
    work(s, move |service| {
        service.store().resolve_markup(&project, &id, &markup)?;
        Ok(json!({"resolved":true}))
    })
    .await
}

async fn head(State(s): State<Shared>, Path((project, id)): Path<(String, String)>) -> Response {
    work(s, move |service| {
        Ok(json!(service.store().get(&project, &id)?))
    })
    .await
}

#[cfg(test)]
mod direct_tests {
    use super::*;
    use axum::{body::Body, http::Request};
    use http_body_util::BodyExt;
    use tower::ServiceExt;
    async fn request(app: Router, method: &str, path: &str, body: Value) -> (u16, Value) {
        let response = app
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(path)
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status().as_u16();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }
    #[tokio::test]
    async fn direct_operations_are_visible_and_retired_routes_are_absent() {
        let mut store = Store::in_memory().unwrap();
        store.create_project("p", "Project").unwrap();
        let initial = json!({"elements":[],"files":{}});
        let figure = store
            .create("p", "Figure", "excalidraw/v2", &initial)
            .unwrap();
        let app = router(store);
        let base = format!("/projects/p/figures/{}", figure.id);
        let edit = json!({"expected_revision":1,"operations":[{"op":"add_wrapped_text","id":"note","text":"Direct edit","x":0,"y":0,"max_width":300,"font_size":18}]});
        let (status, result) = request(
            app.clone(),
            "POST",
            &format!("{base}/operations"),
            edit.clone(),
        )
        .await;
        assert_eq!(status, 200);
        assert_eq!(result["revision"], 2);
        let (_, current) = request(app.clone(), "GET", &base, Value::Null).await;
        assert_eq!(
            current["scene"]["document"]["elements"][0]["originalText"],
            "Direct edit"
        );
        assert_eq!(
            request(app.clone(), "GET", &format!("{base}/head"), Value::Null)
                .await
                .1["revision"],
            2
        );
        assert_eq!(
            request(app.clone(), "POST", &format!("{base}/operations"), edit)
                .await
                .0,
            409
        );
        assert_eq!(
            request(
                app.clone(),
                "GET",
                &format!("{base}/revisions/1"),
                Value::Null
            )
            .await
            .1["document"],
            initial
        );
        for path in [
            format!("{base}/proposals"),
            format!("{base}/proposal-summary"),
            "/proposal-preview-requests".into(),
        ] {
            assert_eq!(request(app.clone(), "GET", &path, Value::Null).await.0, 404);
        }
        assert_eq!(
            request(app, "POST", &format!("{base}/proposals"), json!({}))
                .await
                .0,
            404
        );
    }
}
