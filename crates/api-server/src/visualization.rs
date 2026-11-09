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
        ServiceError::Store(visualization::Error::Invalid(_)) | ServiceError::Scene(_) => {
            StatusCode::BAD_REQUEST
        }
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
            "/projects/{project}/figures/{id}/proposals",
            get(proposals).post(propose),
        )
        .route(
            "/projects/{project}/figures/{id}/proposals/{proposal}/accept",
            post(accept),
        )
        .route(
            "/projects/{project}/figures/{id}/proposals/{proposal}/reject",
            post(reject),
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
    work(s,move |service|Ok(json!({"metadata":service.store().get(&project,&id)?,"scene":service.current(&project,&id)?,"annotations":service.store().annotations(&project,&id)?}))).await
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

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{body::Body, http::Request};
    use http_body_util::BodyExt;
    use serde_json::{Value, json};
    use tower::ServiceExt;
    async fn request(router: axum::Router, method: &str, path: &str, body: Value) -> (u16, Value) {
        let response = router
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
        (status, serde_json::from_slice(&bytes).unwrap())
    }
    #[tokio::test]
    async fn history_lists_metadata_and_restores_as_a_new_revision() {
        let app = router(visualization::Store::in_memory().unwrap());
        request(
            app.clone(),
            "POST",
            "/projects",
            json!({"id":"p","title":"Study"}),
        )
        .await;
        let (_, figure) = request(
            app.clone(),
            "POST",
            "/projects/p/figures",
            json!({"title":"Original","format":"excalidraw/v2","document":{"elements":[]}}),
        )
        .await;
        let path = format!("/projects/p/figures/{}", figure["id"].as_str().unwrap());
        request(
            app.clone(),
            "PUT",
            &path,
            json!({"expected_revision":1,"title":"Edited","document":{"elements":[{"id":"cell"}]}}),
        )
        .await;
        let (status, history) = request(
            app.clone(),
            "GET",
            &format!("{path}/revisions?limit=1"),
            Value::Null,
        )
        .await;
        assert_eq!(status, 200);
        assert_eq!(history[0]["revision"], 2);
        assert!(history[0].get("document").is_none());
        assert_eq!(
            request(
                app.clone(),
                "GET",
                &format!("{path}/revisions?before=2"),
                Value::Null
            )
            .await
            .1[0]["revision"],
            1
        );
        let (_, old) = request(
            app.clone(),
            "GET",
            &format!("{path}/revisions/1"),
            Value::Null,
        )
        .await;
        let (status, restored) = request(
            app.clone(),
            "PUT",
            &path,
            json!({"expected_revision":2,"title":old["title"],"document":old["document"]}),
        )
        .await;
        assert_eq!(status, 200);
        assert_eq!(restored["revision"], 3);
        assert_eq!(
            request(app, "GET", &format!("{path}/revisions/2"), Value::Null)
                .await
                .1["title"],
            "Edited"
        );
    }
    #[tokio::test]
    async fn proposal_acceptance_is_version_checked() {
        let app = router(visualization::Store::in_memory().unwrap());
        request(
            app.clone(),
            "POST",
            "/projects",
            json!({"id":"p","title":"Study"}),
        )
        .await;
        let (_, figure) = request(
            app.clone(),
            "POST",
            "/projects/p/figures",
            json!({"title":"Cell","format":"excalidraw/v2","document":{"elements":[]}}),
        )
        .await;
        let path = format!("/projects/p/figures/{}", figure["id"].as_str().unwrap());
        let (status,proposal) = request(app.clone(), "POST", &format!("{path}/proposals"), json!({"expected_revision":1,"summary":"Add cell","operations":[{"op":"add","element":{"id":"cell"}}]})).await;
        assert_eq!(status, 200);
        assert_eq!(
            request(app.clone(), "GET", &path, Value::Null).await.1["metadata"]["revision"],
            1
        );
        let accept = format!(
            "{path}/proposals/{}/accept",
            proposal["id"].as_str().unwrap()
        );
        assert_eq!(
            request(app.clone(), "POST", &accept, Value::Null).await.1["revision"],
            2
        );
        assert_eq!(
            request(app.clone(), "POST", &accept, Value::Null).await.0,
            409
        );
        let png = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVQIHWP4z8DwHwAFgAI/ScLbtAAAAABJRU5ErkJggg==";
        let preview_path = format!("{path}/revisions/2/preview");
        assert_eq!(
            request(app.clone(), "PUT", &preview_path, json!({"dataURL":png}))
                .await
                .0,
            200
        );
        assert_eq!(
            request(app.clone(), "GET", &preview_path, Value::Null)
                .await
                .1["dataURL"],
            png
        );
        assert_eq!(
            request(
                app.clone(),
                "PUT",
                &format!("{path}/revisions/1/preview"),
                json!({"dataURL":png})
            )
            .await
            .0,
            409
        );
        let (status, note) = request(
            app.clone(),
            "POST",
            &format!("{path}/annotations"),
            json!({"expected_revision":2,"element_ids":["cell"],"note":"Emphasize this cell"}),
        )
        .await;
        assert_eq!(status, 200);
        assert_eq!(
            request(
                app.clone(),
                "GET",
                &format!("{path}/annotations"),
                Value::Null
            )
            .await
            .1[0]["note"],
            "Emphasize this cell"
        );
        assert_eq!(
            request(
                app.clone(),
                "POST",
                &format!(
                    "{path}/annotations/{}/resolve",
                    note["id"].as_str().unwrap()
                ),
                Value::Null
            )
            .await
            .0,
            200
        );
        assert_eq!(
            request(app.clone(), "GET", &path, Value::Null).await.1["metadata"]["revision"],
            2
        );
        assert_eq!(
            request(app, "GET", &format!("{path}/proposals"), Value::Null)
                .await
                .1[0]["status"],
            "accepted"
        );
    }
    #[tokio::test]
    async fn project_and_revision_flow() {
        let app = router(visualization::Store::in_memory().unwrap());
        assert_eq!(
            request(
                app.clone(),
                "POST",
                "/projects",
                json!({"id":"p","title":"Study"})
            )
            .await
            .0,
            200
        );
        let (status, created) = request(
            app.clone(),
            "POST",
            "/projects/p/figures",
            json!({"title":"Cell","format":"excalidraw/v2","document":{"elements":[]}}),
        )
        .await;
        assert_eq!(status, 200);
        let path = format!("/projects/p/figures/{}", created["id"].as_str().unwrap());
        assert_eq!(request(app.clone(),"POST",&format!("{path}/operations"),json!({"expected_revision":1,"operations":[{"op":"add","element":{"id":"cell","type":"ellipse"}}]})).await.1["revision"],2);
        assert_eq!(
            request(
                app.clone(),
                "POST",
                &format!("{path}/operations"),
                json!({"expected_revision":1,"operations":[]})
            )
            .await
            .0,
            409
        );
        assert_eq!(
            request(
                app.clone(),
                "GET",
                &path.replace("/p/", "/other/"),
                Value::Null
            )
            .await
            .0,
            404
        );
        assert_eq!(
            request(
                app.clone(),
                "GET",
                &format!("{path}/revisions/1"),
                Value::Null
            )
            .await
            .1["document"]["elements"],
            json!([])
        );
        assert_eq!(
            request(app, "GET", &path, Value::Null).await.1["scene"]["document"]["elements"][0]["id"],
            "cell"
        );
    }
}

#[derive(Deserialize)]
struct ProposalInput {
    expected_revision: i64,
    summary: String,
    operations: Vec<visualization::scene::Operation>,
}
async fn proposals(
    State(s): State<Shared>,
    Path((project, id)): Path<(String, String)>,
) -> Response {
    work(s, move |service| {
        Ok(json!(service.store().proposals(&project, &id)?))
    })
    .await
}
async fn propose(
    State(s): State<Shared>,
    Path((project, id)): Path<(String, String)>,
    Json(input): Json<ProposalInput>,
) -> Response {
    if input.operations.len() > 1000 {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({"error":"at most 1000 operations per batch"})),
        ));
    }
    work(s, move |service| {
        Ok(json!(service.propose(
            &project,
            &id,
            input.expected_revision,
            &input.summary,
            &input.operations
        )?))
    })
    .await
}
async fn accept(
    State(s): State<Shared>,
    Path((project, id, proposal)): Path<(String, String, String)>,
) -> Response {
    work(s, move |service| {
        Ok(json!(
            service
                .store_mut()
                .accept_proposal(&project, &id, &proposal)?
        ))
    })
    .await
}
async fn reject(
    State(s): State<Shared>,
    Path((project, id, proposal)): Path<(String, String, String)>,
) -> Response {
    work(s, move |service| {
        service.store().reject_proposal(&project, &id, &proposal)?;
        Ok(json!({"rejected":true}))
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
