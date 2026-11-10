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
            "/projects/{project}/figures/{id}/proposals/{proposal}",
            get(read_proposal),
        )
        .route(
            "/projects/{project}/figures/{id}/proposals/{proposal}/revisions",
            post(revise_proposal),
        )
        .route(
            "/projects/{project}/figures/{id}/proposal-preview-requests",
            get(proposal_preview_requests),
        )
        .route(
            "/projects/{project}/figures/{id}/proposals/{proposal}/preview-request",
            post(request_proposal_preview),
        )
        .route(
            "/projects/{project}/figures/{id}/proposals/{proposal}/preview",
            get(read_proposal_preview).post(complete_proposal_preview),
        )
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
        .route(
            "/projects/{project}/figures/{id}/planning",
            get(planning).put(save_planning),
        )
        .route(
            "/projects/{project}/figures/{id}/planning/revisions/{version}",
            get(planning_revision),
        )
        .route(
            "/projects/{project}/figures/{id}/planning/sketch",
            post(planning_sketch),
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
    async fn proposal_preview_http_is_content_bound_and_does_not_adopt() {
        let mut store = Store::in_memory().unwrap();
        store.create_project("p", "Study").unwrap();
        store.create_project("other", "Other").unwrap();
        let figure = store
            .create("p", "Figure", "excalidraw/v2", &json!({"elements":[]}))
            .unwrap();
        let p = store
            .propose(
                "p",
                &figure.id,
                1,
                "Candidate",
                &json!({"elements":[{"id":"candidate","type":"rectangle"}]}),
            )
            .unwrap();
        let app = router(store);
        let path = format!("/projects/p/figures/{}", figure.id);
        let preview = format!("{path}/proposals/{}/preview", p.id);
        let (status, queued) = request(
            app.clone(),
            "POST",
            &format!("{preview}-request"),
            json!({"expected_revision":1}),
        )
        .await;
        assert_eq!(status, 200);
        assert_eq!(queued["render_status"], "requested");
        let (status, jobs) = request(
            app.clone(),
            "GET",
            &format!("{path}/proposal-preview-requests"),
            Value::Null,
        )
        .await;
        assert_eq!(status, 200);
        assert_eq!(jobs[0]["document"], p.document);
        let png = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVQIHWP4z8DwHwAFgAI/ScLbtAAAAABJRU5ErkJggg==";
        let upload = json!({"base_revision":1,"document_sha256":queued["document_sha256"],"dataURL":png,"renderer":"test-renderer"});
        let mut wrong = upload.clone();
        wrong["document_sha256"] = json!("wrong");
        assert_eq!(request(app.clone(), "POST", &preview, wrong).await.0, 409);
        assert_eq!(request(app.clone(), "POST", &preview, upload).await.0, 200);
        let ready = request(
            app.clone(),
            "GET",
            &format!("{preview}?expected_revision=1"),
            Value::Null,
        )
        .await
        .1;
        assert_eq!(ready["scope"], "proposal_candidate");
        assert_eq!(ready["dataURL"], png);
        assert_eq!(ready["proposal_status"], "pending");
        assert_eq!(
            request(app.clone(), "GET", &path, Value::Null).await.1["metadata"]["revision"],
            1
        );
        assert_eq!(
            request(
                app.clone(),
                "GET",
                &format!(
                    "/projects/other/figures/{}/proposal-preview-requests",
                    figure.id
                ),
                Value::Null
            )
            .await
            .0,
            404
        );
        assert_eq!(
            request(
                app,
                "GET",
                &format!("{preview}?expected_revision=0"),
                Value::Null
            )
            .await
            .0,
            409
        );
    }
    #[tokio::test]
    async fn inset_http_validates_revision_and_requires_review() {
        let mut store = Store::in_memory().unwrap();

        use base64::Engine;
        let url=format!("data:image/svg+xml;base64,{}",base64::engine::general_purpose::STANDARD.encode(b"<svg xmlns='http://www.w3.org/2000/svg' width='100' height='100'><path d='M0 0h100v100H0z'/></svg>"));
        let (scene, target) = visualization::image::insert(
            &json!({"elements":[]}),
            visualization::image::Image {
                file_id: "source",
                data_url: &url,
                width: 100,
                height: 100,
                provenance: json!({"asset":{"license":"cc0"}}),
            },
            0.,
            0.,
        )
        .unwrap();
        store.create_project("p", "Study").unwrap();
        let figure = store
            .create("p", "Figure", "excalidraw/v2", &scene)
            .unwrap();
        let spec = json!({"element_id":target,"region":{"x":0.1,"y":0.2,"width":0.4,"height":0.5},"x":600,"y":100,"max_size":320,"title":"Detail"});

        let app = router(store);
        let path = format!("/projects/p/figures/{}", figure.id);
        assert_eq!(
            request(
                app.clone(),
                "GET",
                &format!("{path}/image-info?element_id={target}&expected_revision=1"),
                Value::Null
            )
            .await
            .1["natural_width"],
            100
        );
        let input = json!({"expected_revision":1,"inset":spec});
        let (status, proposal) = request(
            app.clone(),
            "POST",
            &format!("{path}/insets"),
            input.clone(),
        )
        .await;
        assert_eq!(status, 200);
        assert_eq!(proposal["document"]["elements"][0], scene["elements"][0]);
        assert_eq!(
            request(app.clone(), "GET", &path, Value::Null).await.1["metadata"]["revision"],
            1
        );
        let mut stale = input;
        stale["expected_revision"] = json!(0);
        assert_eq!(
            request(app.clone(), "POST", &format!("{path}/insets"), stale)
                .await
                .0,
            409
        );
        assert_eq!(
            request(
                app.clone(),
                "GET",
                &format!(
                    "/projects/other/figures/{}/image-info?element_id={target}&expected_revision=1",
                    figure.id
                ),
                Value::Null
            )
            .await
            .0,
            404
        );

        let accept = format!(
            "{path}/proposals/{}/accept",
            proposal["id"].as_str().unwrap()
        );
        assert_eq!(
            request(app.clone(), "POST", &accept, Value::Null).await.0,
            200
        );
        let anchor = proposal["document"]["elements"][3]["id"].as_str().unwrap();
        let (status, checked) = request(
            app.clone(),
            "GET",
            &format!("{path}/inset-status?element_id={anchor}&expected_revision=2"),
            Value::Null,
        )
        .await;
        assert_eq!(status, 200);
        assert_eq!(checked["changed"], false);
        let (status, refreshed) = request(
            app.clone(),
            "POST",
            &format!("{path}/insets/refresh"),
            json!({"element_id":anchor,"expected_revision":2}),
        )
        .await;
        assert_eq!(status, 200);
        assert_eq!(refreshed["base_revision"], 2);
        assert_eq!(refreshed["document"]["files"], scene["files"]);
        assert_eq!(
            request(
                app,
                "POST",
                &format!("{path}/insets/refresh"),
                json!({"element_id":anchor,"expected_revision":1})
            )
            .await
            .0,
            409
        );
    }
    #[tokio::test]
    async fn scientific_planning_persists_clarification_before_a_reviewable_sketch() {
        let app = router(Store::in_memory().unwrap());
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
            json!({"title":"Figure","format":"excalidraw/v2","document":{"elements":[]}}),
        )
        .await;
        let path = format!("/projects/p/figures/{}", figure["id"].as_str().unwrap());
        assert_eq!(
            request(app.clone(), "GET", &format!("{path}/planning"), Value::Null)
                .await
                .1,
            Value::Null
        );
        let input = json!({"expected_revision":1,"expected_plan_version":0,"document":{"message":"Conceptual cell","audience":"Researchers","nodes":[{"id":"cell","label":"Cell"}],"questions":[{"id":"q1","question":"Which cell?"}]}});
        let (status, plan) = request(
            app.clone(),
            "PUT",
            &format!("{path}/planning"),
            input.clone(),
        )
        .await;
        assert_eq!(status, 200);
        assert_eq!(plan["status"], "needs_clarification");
        let generate =
            json!({"expected_revision":1,"expected_plan_version":1,"summary":"Planned sketch"});
        assert_eq!(
            request(
                app.clone(),
                "POST",
                &format!("{path}/planning/sketch"),
                generate.clone()
            )
            .await
            .0,
            400
        );
        let mut edited = input.clone();
        edited["expected_plan_version"] = json!(1);
        edited["document"]["questions"][0]["answer"] = json!("Generic conceptual cell");
        assert_eq!(
            request(app.clone(), "PUT", &format!("{path}/planning"), edited)
                .await
                .1["status"],
            "ready"
        );
        assert_eq!(
            request(app.clone(), "PUT", &format!("{path}/planning"), input)
                .await
                .0,
            409
        );
        let (_, historical) = request(
            app.clone(),
            "GET",
            &format!("{path}/planning/revisions/1"),
            Value::Null,
        )
        .await;
        assert_eq!(historical["document"]["questions"][0]["answer"], "");
        let mut generate = generate;
        generate["expected_plan_version"] = json!(2);
        let (status, proposal) = request(
            app.clone(),
            "POST",
            &format!("{path}/planning/sketch"),
            generate,
        )
        .await;
        assert_eq!(status, 200);
        assert!(proposal.get("document").is_none());
        assert_eq!(
            request(app.clone(), "GET", &path, Value::Null).await.1["metadata"]["revision"],
            1
        );
        assert_eq!(
            request(
                app.clone(),
                "GET",
                &format!("{path}/planning").replace("/p/", "/other/"),
                Value::Null
            )
            .await
            .0,
            404
        );
        assert_eq!(
            request(
                app.clone(),
                "POST",
                &format!(
                    "{path}/proposals/{}/accept",
                    proposal["id"].as_str().unwrap()
                ),
                Value::Null
            )
            .await
            .0,
            200
        );
        assert_eq!(
            request(app, "GET", &path, Value::Null).await.1["scene"]["document"]["elements"][0]["customData"]
                ["planningSource"]["version"],
            2
        );
    }
    #[tokio::test]
    async fn markups_are_separate_version_bound_review_records() {
        let app = router(Store::in_memory().unwrap());
        request(
            app.clone(),
            "POST",
            "/projects",
            json!({"id":"p","title":"Study"}),
        )
        .await;
        let (_, figure) = request(app.clone(), "POST", "/projects/p/figures", json!({"title":"Figure","format":"excalidraw/v2","document":{"elements":[{"id":"cell"}]}})).await;
        let path = format!("/projects/p/figures/{}", figure["id"].as_str().unwrap());
        let png = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVQIHWP4z8DwHwAFgAI/ScLbtAAAAABJRU5ErkJggg==";
        let input = json!({"expected_revision":1,"note":"Remove crossed component","strokes":[{"kind":"cross","points":[[10,10],[40,40]]}],"dataURL":png});
        let (status, review) = request(
            app.clone(),
            "POST",
            &format!("{path}/markups"),
            input.clone(),
        )
        .await;
        assert_eq!(status, 200);
        assert!(review.get("dataURL").is_none());
        let review_path = format!("{path}/markups/{}", review["id"].as_str().unwrap());
        let (_, page) = request(
            app.clone(),
            "GET",
            &format!("{path}/markups?page=true&status=open&limit=1"),
            Value::Null,
        )
        .await;
        assert_eq!(page["items"][0]["id"], review["id"]);
        assert!(page["next_before"].is_null());
        assert!(page["items"][0].get("dataURL").is_none());
        for query in ["limit=0", "status=unknown", "limit=101"] {
            assert_eq!(
                request(
                    app.clone(),
                    "GET",
                    &format!("{path}/markups?{query}"),
                    Value::Null
                )
                .await
                .0,
                400
            );
        }
        assert_eq!(
            request(
                app.clone(),
                "GET",
                &format!("{path}/markups?page=true&before=missing"),
                Value::Null
            )
            .await
            .0,
            404
        );

        assert_eq!(
            request(app.clone(), "GET", &review_path, Value::Null)
                .await
                .1["dataURL"],
            png
        );
        assert_eq!(
            request(
                app.clone(),
                "GET",
                &review_path.replace("/p/", "/other/"),
                Value::Null
            )
            .await
            .0,
            404
        );
        assert_eq!(
            request(app.clone(), "GET", &path, Value::Null).await.1["metadata"]["revision"],
            1
        );
        request(
            app.clone(),
            "PUT",
            &path,
            json!({"expected_revision":1,"title":"Edited","document":{"elements":[]}}),
        )
        .await;
        assert_eq!(
            request(app.clone(), "POST", &format!("{path}/markups"), input)
                .await
                .0,
            409
        );
        assert_eq!(
            request(app.clone(), "GET", &review_path, Value::Null)
                .await
                .1["at_revision"],
            1
        );
        assert_eq!(
            request(
                app.clone(),
                "POST",
                &format!("{review_path}/resolve"),
                Value::Null
            )
            .await
            .0,
            200
        );
        assert_eq!(
            request(app.clone(), "GET", &format!("{path}/markups"), Value::Null)
                .await
                .1[0]["status"],
            "resolved"
        );
        assert_eq!(
            request(app, "GET", &format!("{path}/revisions/1"), Value::Null)
                .await
                .1["document"]["elements"][0]["id"],
            "cell"
        );
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
    async fn linked_review_completion_requires_explicit_adoption() {
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
            json!({"title":"Figure","format":"excalidraw/v2","document":{"elements":[]}}),
        )
        .await;
        let path = format!("/projects/p/figures/{}", figure["id"].as_str().unwrap());
        let png = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVQIHWP4z8DwHwAFgAI/ScLbtAAAAABJRU5ErkJggg==";
        let (_, review) = request(app.clone(), "POST", &format!("{path}/markups"), json!({"expected_revision":1,"note":"Add step","strokes":[{"kind":"draw","points":[[0,0],[10,10]]}],"dataURL":png})).await;
        let input = json!({"expected_revision":1,"summary":"Address review","operations":[{"op":"add","element":{"id":"step","type":"rectangle"}}],"markup_ids":[review["id"]]});
        let (status, proposal) =
            request(app.clone(), "POST", &format!("{path}/proposals"), input).await;
        assert_eq!(status, 200);
        assert_eq!(proposal["review_markups"][0]["note"], "Add step");
        assert!(!proposal.to_string().contains("iVBOR"));
        let (_, notes) = request(app.clone(), "GET", &format!("{path}/markups"), Value::Null).await;
        assert_eq!(notes[0]["status"], "open");
        let accept = format!(
            "{path}/proposals/{}/accept?resolve_markups=true",
            proposal["id"].as_str().unwrap()
        );
        assert_eq!(
            request(app.clone(), "POST", &accept, Value::Null).await.1["revision"],
            2
        );
        let (_, notes) = request(app.clone(), "GET", &format!("{path}/markups"), Value::Null).await;
        assert_eq!(notes[0]["status"], "resolved");
        assert_eq!(request(app, "POST", &accept, Value::Null).await.0, 409);
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
    #[serde(default)]
    markup_ids: Vec<String>,
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
        Ok(json!(service.propose_with_markups(
            &project,
            &id,
            input.expected_revision,
            &input.summary,
            &input.operations,
            &input.markup_ids
        )?))
    })
    .await
}
#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct AcceptanceQuery {
    #[serde(default)]
    resolve_markups: bool,
}
async fn accept(
    State(s): State<Shared>,
    Path((project, id, proposal)): Path<(String, String, String)>,
    Query(query): Query<AcceptanceQuery>,
) -> Response {
    work(s, move |service| {
        Ok(json!(service.store_mut().accept_proposal_with_reviews(
            &project,
            &id,
            &proposal,
            query.resolve_markups
        )?))
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

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PlanningInput {
    expected_revision: i64,
    expected_plan_version: i64,
    document: visualization::planning::Brief,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PlanningSketchInput {
    expected_revision: i64,
    expected_plan_version: i64,
    summary: String,
}
async fn planning(
    State(s): State<Shared>,
    Path((project, id)): Path<(String, String)>,
) -> Response {
    work(s, move |service| {
        Ok(json!(service.store().planning(&project, &id)?))
    })
    .await
}
async fn planning_revision(
    State(s): State<Shared>,
    Path((project, id, version)): Path<(String, String, i64)>,
) -> Response {
    work(s, move |service| {
        Ok(json!(
            service.store().planning_revision(&project, &id, version)?
        ))
    })
    .await
}
async fn save_planning(
    State(s): State<Shared>,
    Path((project, id)): Path<(String, String)>,
    Json(input): Json<PlanningInput>,
) -> Response {
    work(s, move |service| {
        Ok(json!(service.store_mut().save_planning(
            &project,
            &id,
            input.expected_revision,
            input.expected_plan_version,
            &input.document
        )?))
    })
    .await
}
async fn planning_sketch(
    State(s): State<Shared>,
    Path((project, id)): Path<(String, String)>,
    Json(input): Json<PlanningSketchInput>,
) -> Response {
    work(s, move |service| {
        let mut proposal = json!(service.store_mut().planned_proposal(
            &project,
            &id,
            input.expected_revision,
            input.expected_plan_version,
            &input.summary
        )?);
        proposal.as_object_mut().unwrap().remove("document");
        Ok(proposal)
    })
    .await
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CandidatePreviewQuery {
    expected_revision: i64,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CandidatePreviewRequest {
    expected_revision: i64,
    #[serde(default)]
    retry_failed: bool,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CandidatePreviewCompletion {
    base_revision: i64,
    document_sha256: String,
    renderer: String,
    #[serde(rename = "dataURL")]
    data_url: Option<String>,
    error: Option<String>,
}
async fn proposal_preview_requests(
    State(s): State<Shared>,
    Path((project, id)): Path<(String, String)>,
) -> Response {
    work(s, move |service| {
        Ok(json!(
            service.store().proposal_render_requests(&project, &id)?
        ))
    })
    .await
}
async fn request_proposal_preview(
    State(s): State<Shared>,
    Path((project, id, proposal)): Path<(String, String, String)>,
    Json(input): Json<CandidatePreviewRequest>,
) -> Response {
    work(s, move |service| {
        Ok(json!(service.store().request_proposal_preview(
            &project,
            &id,
            &proposal,
            input.expected_revision,
            input.retry_failed
        )?))
    })
    .await
}
async fn read_proposal_preview(
    State(s): State<Shared>,
    Path((project, id, proposal)): Path<(String, String, String)>,
    Query(input): Query<CandidatePreviewQuery>,
) -> Response {
    work(s, move |service| {
        Ok(json!(service.store().proposal_preview(
            &project,
            &id,
            &proposal,
            input.expected_revision
        )?))
    })
    .await
}
async fn complete_proposal_preview(
    State(s): State<Shared>,
    Path((project, id, proposal)): Path<(String, String, String)>,
    Json(input): Json<CandidatePreviewCompletion>,
) -> Response {
    work(s, move |service| {
        service.store().complete_proposal_preview(
            &project,
            &id,
            &proposal,
            visualization::proposal_preview::PreviewCompletion {
                base_revision: input.base_revision,
                document_sha256: &input.document_sha256,
                data_url: input.data_url.as_deref(),
                error: input.error.as_deref(),
                renderer: &input.renderer,
            },
        )?;
        Ok(json!({"ok":true}))
    })
    .await
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReviseProposalInput {
    expected_revision: i64,
    parent_document_sha256: String,
    summary: String,
    operations: Option<Vec<visualization::scene::Operation>>,
    arrangement: Option<visualization::arrange::Spec>,
    markup_ids: Option<Vec<String>>,
}
async fn read_proposal(
    State(s): State<Shared>,
    Path((project, id, proposal)): Path<(String, String, String)>,
) -> Response {
    work(s, move |service| {
        Ok(json!(service.store().proposal(&project, &id, &proposal)?))
    })
    .await
}
async fn revise_proposal(
    State(s): State<Shared>,
    Path((project, id, proposal)): Path<(String, String, String)>,
    Json(input): Json<ReviseProposalInput>,
) -> Response {
    work(s, move |service| {
        Ok(json!(service.revise_proposal(
            &project,
            &id,
            input.expected_revision,
            &visualization::service::ProposalRevisionInput {
                parent_proposal_id: proposal,
                parent_document_sha256: input.parent_document_sha256,
                summary: input.summary,
                operations: input.operations,
                arrangement: input.arrangement,
                markup_ids: input.markup_ids
            }
        )?))
    })
    .await
}
