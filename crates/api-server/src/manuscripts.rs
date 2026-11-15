//! Bibliography-owned source editor and human review API.
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Path, Query, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, patch, post, put},
};
use base64::Engine as _;
use bib_base::BibShared;
use manuscript_core::{Error, ProjectPatch};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, sync::Arc};

type Shared = State<Arc<BibShared>>;
type ApiResult = Result<Json<Value>, ApiError>;
struct ApiError(Error);
impl From<Error> for ApiError {
    fn from(e: Error) -> Self {
        Self(e)
    }
}
impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = match self.0 {
            Error::NotFound => StatusCode::NOT_FOUND,
            Error::Conflict => StatusCode::CONFLICT,
            Error::Invalid(_) => StatusCode::BAD_REQUEST,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        };
        (status, Json(json!({"error":self.0.to_string()}))).into_response()
    }
}
fn value(v: impl serde::Serialize) -> ApiResult {
    Ok(Json(serde_json::to_value(v).map_err(Error::from)?))
}
pub fn router() -> Router<Arc<BibShared>> {
    Router::new()
        .route("/manuscripts", get(list).post(create))
        .route("/manuscripts/{id}", get(detail).patch(update))
        .route("/manuscripts/{id}/files", put(save).delete(remove))
        .route(
            "/manuscripts/{id}/assets",
            put(save_asset).delete(delete_asset).get(asset_bytes),
        )
        .route("/manuscripts/{id}/history", get(history))
        .route("/manuscripts/{id}/restore", post(restore))
        .route("/manuscripts/{id}/proposals", post(propose))
        .route(
            "/manuscripts/{id}/proposals/{proposal}/resolve",
            post(resolve),
        )
        .route("/manuscripts/{id}/comments", post(comment))
        .route(
            "/manuscripts/{id}/comments/{comment}",
            patch(resolve_comment),
        )
        .route("/manuscripts/{id}/references", post(references))
        .route("/manuscripts/{id}/compile", post(compile))
        .route("/manuscripts/{id}/export", get(export))
        .route("/manuscripts/{id}/builds/latest", get(latest_build))
        .route("/manuscripts/{id}/builds/{build}/pdf", get(pdf))
        .layer(DefaultBodyLimit::max(24 * 1024 * 1024))
}
async fn list(State(s): Shared) -> ApiResult {
    value(s.manuscripts.list_projects().await?)
}
#[derive(Deserialize)]
struct Create {
    title: String,
}
async fn create(State(s): Shared, Json(b): Json<Create>) -> ApiResult {
    value(s.manuscripts.create_project(&b.title).await?)
}
async fn detail(State(s): Shared, Path(id): Path<String>) -> ApiResult {
    value(s.manuscripts.detail(&id).await?)
}
async fn update(
    State(s): Shared,
    Path(id): Path<String>,
    Json(b): Json<ProjectPatch>,
) -> ApiResult {
    value(s.manuscripts.update_project(&id, b).await?)
}
#[derive(Deserialize)]
struct Save {
    path: String,
    content: String,
    expected_revision: u64,
}
async fn save(State(s): Shared, Path(id): Path<String>, Json(b): Json<Save>) -> ApiResult {
    value(
        s.manuscripts
            .save_file(
                &id,
                &b.path,
                &b.content,
                b.expected_revision,
                "human",
                "Edit source",
            )
            .await?,
    )
}
#[derive(Deserialize)]
struct Remove {
    path: String,
    expected_revision: u64,
}
async fn remove(State(s): Shared, Path(id): Path<String>, Json(b): Json<Remove>) -> ApiResult {
    s.manuscripts
        .delete_file(&id, &b.path, b.expected_revision)
        .await?;
    value(json!({"deleted":true}))
}
#[derive(Deserialize)]
struct History {
    path: Option<String>,
}
async fn history(State(s): Shared, Path(id): Path<String>, Query(b): Query<History>) -> ApiResult {
    value(s.manuscripts.history(&id, b.path.as_deref()).await?)
}
#[derive(Deserialize)]
struct Restore {
    path: String,
    revision: u64,
    expected_revision: u64,
}
async fn restore(State(s): Shared, Path(id): Path<String>, Json(b): Json<Restore>) -> ApiResult {
    value(
        s.manuscripts
            .restore(&id, &b.path, b.revision, b.expected_revision)
            .await?,
    )
}
#[derive(Deserialize)]
struct Propose {
    path: String,
    content: String,
    base_revision: u64,
    summary: String,
}
async fn propose(State(s): Shared, Path(id): Path<String>, Json(b): Json<Propose>) -> ApiResult {
    value(
        s.manuscripts
            .propose(
                &id,
                &b.path,
                &b.content,
                b.base_revision,
                &b.summary,
                "human",
            )
            .await?,
    )
}
#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
enum Action {
    Accept,
    Reject,
}
#[derive(Deserialize)]
struct Resolve {
    action: Action,
}
async fn resolve(
    State(s): Shared,
    Path((id, proposal)): Path<(String, String)>,
    Json(b): Json<Resolve>,
) -> ApiResult {
    value(
        s.manuscripts
            .resolve_proposal(&id, &proposal, matches!(b.action, Action::Accept))
            .await?,
    )
}
#[derive(Deserialize)]
struct CommentInput {
    path: String,
    start_line: usize,
    end_line: usize,
    body: String,
    expected_revision: Option<u64>,
}
async fn comment(
    State(s): Shared,
    Path(id): Path<String>,
    Json(b): Json<CommentInput>,
) -> ApiResult {
    value(
        s.manuscripts
            .add_comment_at_revision(
                &id,
                &b.path,
                b.start_line,
                b.end_line,
                &b.body,
                "human",
                b.expected_revision,
            )
            .await?,
    )
}
#[derive(Deserialize)]
struct ResolveComment {
    resolved: bool,
}
async fn resolve_comment(
    State(s): Shared,
    Path((id, comment)): Path<(String, String)>,
    Json(b): Json<ResolveComment>,
) -> ApiResult {
    value(
        s.manuscripts
            .resolve_comment(&id, &comment, b.resolved)
            .await?,
    )
}
#[derive(Deserialize)]
struct References {
    article_ids: Vec<String>,
    expected_revision: u64,
}
async fn references(
    State(s): Shared,
    Path(id): Path<String>,
    Json(b): Json<References>,
) -> ApiResult {
    if b.article_ids.len() > 2000 {
        return Err(Error::Invalid("at most 2000 references per project".into()).into());
    }
    let mut keys = BTreeMap::new();
    let mut entries = vec![];
    let mut ids = vec![];
    for id in b.article_ids {
        if keys.contains_key(&id) {
            continue;
        }
        let article = s
            .bib
            .get_article(&id)
            .await
            .map_err(|e| Error::Invalid(e.to_string()))?
            .ok_or(Error::NotFound)?;
        // Library IDs, unlike titles/authors, are stable across metadata editing.
        let key = format!("ref{:x}", Sha256::digest(id.as_bytes()));
        let bib = bib_base::export::to_bibtex(&article);
        let open = bib
            .find('{')
            .ok_or_else(|| Error::Invalid("invalid bibliography entry".into()))?;
        let comma = bib
            .find(',')
            .ok_or_else(|| Error::Invalid("invalid bibliography entry".into()))?;
        entries.push(format!("{}{}{}", &bib[..=open], key, &bib[comma..]));
        keys.insert(id.clone(), key);
        ids.push(id);
    }
    let file = s
        .manuscripts
        .sync_references(&id, ids, &entries.join("\n\n"), b.expected_revision)
        .await?;
    value(json!({"file":file,"keys":keys}))
}
static COMPILE_PERMITS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(2);
async fn compile(State(s): Shared, Path(id): Path<String>) -> Result<Json<Value>, Response> {
    let _permit = COMPILE_PERMITS.try_acquire().map_err(|_| {
        (
            StatusCode::TOO_MANY_REQUESTS,
            Json(json!({"error":"Compiler busy; retry shortly"})),
        )
            .into_response()
    })?;
    let snapshot = s
        .manuscripts
        .compile_snapshot(&id)
        .await
        .map_err(|e| ApiError(e).into_response())?;
    let revisions = snapshot.source_revisions;
    let result = manuscript_engine::EngineConfig::from_env()
        .compile(snapshot.files, &snapshot.main_file)
        .await;
    let (success, pdf, log, diagnostics) = match result {
        Ok(output) => (
            output.success,
            output.pdf,
            output.log,
            output
                .diagnostics
                .into_iter()
                .map(|d| serde_json::to_value(d).unwrap_or(Value::Null))
                .collect(),
        ),
        Err(error) => (
            false,
            None,
            error.to_string(),
            vec![json!({"severity":"error","message":error.to_string(),"file":null,"line":null})],
        ),
    };
    let build = manuscript_core::Build {
        main_file: snapshot.main_file,
        id: uuid::Uuid::new_v4().to_string(),
        status: if success { "succeeded" } else { "failed" }.into(),
        log,
        diagnostics,
        source_revisions: revisions,
        created_at: chrono::Utc::now().to_rfc3339(),
    };
    s.manuscripts
        .save_build(&id, &build, pdf)
        .await
        .map_err(|e| ApiError(e).into_response())?;
    value(build).map_err(IntoResponse::into_response)
}
async fn latest_build(State(s): Shared, Path(id): Path<String>) -> ApiResult {
    value(s.manuscripts.latest_build(&id).await?)
}
async fn pdf(
    State(s): Shared,
    Path((id, build)): Path<(String, String)>,
) -> Result<Response, ApiError> {
    let bytes = s.manuscripts.build_pdf(&id, &build).await?;
    Ok((
        [
            (header::CONTENT_TYPE, "application/pdf"),
            (
                header::CONTENT_DISPOSITION,
                "inline; filename=manuscript.pdf",
            ),
            (header::CACHE_CONTROL, "private, no-store"),
        ],
        bytes,
    )
        .into_response())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::{Body, to_bytes},
        http::Request,
    };
    use tower::ServiceExt;
    async fn call(app: &Router, method: &str, path: &str, body: Value) -> (StatusCode, Value) {
        let response = app
            .clone()
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
        let status = response.status();
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }
    #[tokio::test]
    async fn http_rejects_stale_saves_and_cannot_forge_agent_actor() {
        let shared = Arc::new(BibShared::open_in_memory().await.unwrap());
        let project = shared
            .manuscripts
            .create_project("HTTP paper")
            .await
            .unwrap();
        let app = router().with_state(shared.clone());
        let path = format!("/manuscripts/{}/files", project.id);
        let (status,_) = call(&app,"PUT",&path,json!({"path":"main.tex","content":"Human source","expected_revision":1,"actor":"agent:forged"})).await;
        assert_eq!(status, StatusCode::OK);
        let (status, _) = call(
            &app,
            "PUT",
            &path,
            json!({"path":"main.tex","content":"Stale source","expected_revision":1}),
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(
            shared
                .manuscripts
                .history(&project.id, Some("main.tex"))
                .await
                .unwrap()[0]
                .actor,
            "human"
        );
        let (status, _) = call(
            &app,
            "PUT",
            &path,
            json!({"path":"../escape.tex","content":"unsafe","expected_revision":0}),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }
    #[tokio::test]
    async fn proposal_accept_requires_current_base_and_comment_tracks_revision() {
        let shared = Arc::new(BibShared::open_in_memory().await.unwrap());
        let project = shared.manuscripts.create_project("Review").await.unwrap();
        let app = router().with_state(shared.clone());
        let proposal = shared
            .manuscripts
            .propose(
                &project.id,
                "main.tex",
                "Agent proposal",
                1,
                "Rewrite",
                "agent:test",
            )
            .await
            .unwrap();
        let (status, _) = call(
            &app,
            "POST",
            &format!(
                "/manuscripts/{}/proposals/{}/resolve",
                project.id, proposal.id
            ),
            json!({"action":"accept"}),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let (status,comment)=call(&app,"POST",&format!("/manuscripts/{}/comments",project.id),json!({"path":"main.tex","start_line":1,"end_line":1,"body":"Review this","expected_revision":2})).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(comment["revision"], 2);
        let (status,_) = call(&app,"POST",&format!("/manuscripts/{}/comments",project.id),json!({"path":"main.tex","start_line":1,"end_line":1,"body":"stale","expected_revision":1})).await;
        assert_eq!(status, StatusCode::CONFLICT);
    }
    #[tokio::test]
    async fn references_sync_is_atomic_and_keys_survive_metadata_changes() {
        let shared = Arc::new(BibShared::open_in_memory().await.unwrap());
        let p = shared
            .manuscripts
            .create_project("Citations")
            .await
            .unwrap();
        shared
            .bib
            .upsert_article(&bib_types::Article::new("paper-1", "Original title"))
            .await
            .unwrap();
        let app = router().with_state(shared.clone());
        let path = format!("/manuscripts/{}/references", p.id);
        let (status, first) = call(
            &app,
            "POST",
            &path,
            json!({"article_ids":["paper-1"],"expected_revision":1}),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        shared
            .bib
            .upsert_article(&bib_types::Article::new("paper-1", "Corrected title"))
            .await
            .unwrap();
        let (status, second) = call(
            &app,
            "POST",
            &path,
            json!({"article_ids":["paper-1"],"expected_revision":2}),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(first["keys"], second["keys"]);
        let (status, _) = call(
            &app,
            "POST",
            &path,
            json!({"article_ids":["missing"],"expected_revision":3}),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        let detail = shared.manuscripts.detail(&p.id).await.unwrap();
        assert_eq!(detail.project.article_ids, vec!["paper-1"]);
        assert_eq!(
            detail
                .files
                .iter()
                .find(|f| f.path == "references.bib")
                .unwrap()
                .revision,
            3
        );
    }
    /// Requires the actual worker via AUTONOMICS_MANUSCRIPT_WORKER and a provisioned TeX cache.
    #[tokio::test]
    #[ignore = "requires real embedded Tectonic worker and TeX bundle resources"]
    async fn real_http_compile_persists_pdf_with_source_and_asset_revisions() {
        let shared = Arc::new(BibShared::open_in_memory().await.unwrap());
        shared
            .bib
            .upsert_article(&bib_types::Article::new(
                "integration-reference",
                "A real bibliography reference",
            ))
            .await
            .unwrap();
        let app = router().with_state(shared.clone());
        let (status, project) = call(
            &app,
            "POST",
            "/manuscripts",
            json!({"title":"HTTP compilation integration"}),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let id = project["id"].as_str().unwrap();
        let (status, references) = call(
            &app,
            "POST",
            &format!("/manuscripts/{id}/references"),
            json!({"article_ids":["integration-reference"],"expected_revision":1}),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let key = references["keys"]["integration-reference"]
            .as_str()
            .unwrap();
        let source = format!(
            r"\documentclass{{article}}\usepackage{{graphicx}}\begin{{document}}\input{{sections/introduction}}\cite{{{key}}}\includegraphics{{figures/figure.png}}\bibliographystyle{{plain}}\bibliography{{references}}\end{{document}}"
        );
        let (status, _) = call(
            &app,
            "PUT",
            &format!("/manuscripts/{id}/files"),
            json!({"path":"main.tex","content":source,"expected_revision":1}),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let(status,_) = call(&app,"PUT",&format!("/manuscripts/{id}/files"),json!({"path":"sections/introduction.tex","content":"Compiled through the real HTTP snapshot and kernel.\n","expected_revision":0})).await;
        assert_eq!(status, StatusCode::OK);
        let png = include_bytes!("../../manuscript-engine/tests/fixtures/figure.png");
        let(status,_) = call(&app,"PUT",&format!("/manuscripts/{id}/assets"),json!({"path":"figures/figure.png","data_base64":base64::engine::general_purpose::STANDARD.encode(png),"expected_revision":0})).await;
        assert_eq!(status, StatusCode::OK);
        let (status, build) = call(
            &app,
            "POST",
            &format!("/manuscripts/{id}/compile"),
            Value::Null,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(build["status"], "succeeded", "{}", build["log"]);
        assert_eq!(build["main_file"], "main.tex");
        assert_eq!(
            build["source_revisions"],
            json!({"main.tex":2,"references.bib":2,"sections/introduction.tex":1,"figures/figure.png":1})
        );
        let build_id = build["id"].as_str().unwrap();
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/manuscripts/{id}/builds/{build_id}/pdf"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[header::CONTENT_TYPE], "application/pdf");
        let bytes = to_bytes(response.into_body(), 16 * 1024 * 1024)
            .await
            .unwrap();
        assert!(bytes.starts_with(b"%PDF-"));
        assert_eq!(
            bytes.as_ref(),
            shared.manuscripts.build_pdf(id, build_id).await.unwrap()
        );
        let (status, latest) = call(
            &app,
            "GET",
            &format!("/manuscripts/{id}/builds/latest"),
            Value::Null,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(latest, build);
    }
    #[tokio::test]
    async fn source_export_contains_latex_and_binary_assets() {
        use std::io::Read;
        let shared = Arc::new(BibShared::open_in_memory().await.unwrap());
        let p = shared.manuscripts.create_project("Export").await.unwrap();
        shared
            .manuscripts
            .save_asset(&p.id, "figures/chart.png", vec![0, 137, 255], 0)
            .await
            .unwrap();
        let app = router().with_state(shared);
        let (status, body) = call(
            &app,
            "GET",
            &format!("/manuscripts/{}/export", p.id),
            Value::Null,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(body["data_base64"].as_str().unwrap())
            .unwrap();
        let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
        assert!(zip.by_name("main.tex").is_ok());
        assert!(zip.by_name("references.bib").is_ok());
        let mut image = vec![];
        zip.by_name("figures/chart.png")
            .unwrap()
            .read_to_end(&mut image)
            .unwrap();
        assert_eq!(image, vec![0, 137, 255]);
    }
    #[tokio::test]
    async fn build_artifact_is_raw_pdf_and_scoped_to_project() {
        let shared = Arc::new(BibShared::open_in_memory().await.unwrap());
        let p = shared
            .manuscripts
            .create_project("PDF project")
            .await
            .unwrap();
        let build = manuscript_core::Build {
            main_file: "main.tex".into(),
            id: "build-1".into(),
            status: "succeeded".into(),
            log: "compiled".into(),
            diagnostics: vec![],
            source_revisions: BTreeMap::from([("main.tex".into(), 1)]),
            created_at: chrono::Utc::now().to_rfc3339(),
        };
        shared
            .manuscripts
            .save_build(&p.id, &build, Some(b"%PDF-1.7\n".to_vec()))
            .await
            .unwrap();
        let app = router().with_state(shared);
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/manuscripts/{}/builds/build-1/pdf", p.id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[header::CONTENT_TYPE], "application/pdf");
        assert_eq!(
            to_bytes(response.into_body(), 1024).await.unwrap().as_ref(),
            b"%PDF-1.7\n"
        );
        let (status, _) = call(
            &app,
            "GET",
            "/manuscripts/wrong-project/builds/build-1/pdf",
            Value::Null,
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        let (status, body) = call(
            &app,
            "GET",
            &format!("/manuscripts/{}/builds/latest", p.id),
            Value::Null,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["source_revisions"]["main.tex"], 1);
    }
}

#[derive(Deserialize)]
struct AssetUpload {
    path: String,
    data_base64: String,
    expected_revision: u64,
}
async fn save_asset(
    State(s): Shared,
    Path(id): Path<String>,
    Json(b): Json<AssetUpload>,
) -> ApiResult {
    if b.data_base64.len() > 24 * 1024 * 1024 {
        return Err(Error::Invalid("asset exceeds 16 MiB".into()).into());
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(&b.data_base64)
        .map_err(|_| Error::Invalid("invalid base64 asset".into()))?;
    value(
        s.manuscripts
            .save_asset(&id, &b.path, bytes, b.expected_revision)
            .await?,
    )
}
async fn delete_asset(
    State(s): Shared,
    Path(id): Path<String>,
    Json(b): Json<Remove>,
) -> ApiResult {
    s.manuscripts
        .delete_asset(&id, &b.path, b.expected_revision)
        .await?;
    value(json!({"deleted":true}))
}
#[derive(Deserialize)]
struct AssetPath {
    path: String,
}
async fn asset_bytes(
    State(s): Shared,
    Path(id): Path<String>,
    Query(b): Query<AssetPath>,
) -> Result<Response, ApiError> {
    let bytes = s.manuscripts.asset_bytes(&id, &b.path).await?;
    Ok((
        [
            (header::CONTENT_TYPE, "application/octet-stream"),
            (header::CONTENT_DISPOSITION, "attachment"),
            (header::CACHE_CONTROL, "private, no-store"),
        ],
        bytes,
    )
        .into_response())
}

async fn export(State(s): Shared, Path(id): Path<String>) -> ApiResult {
    let snapshot = s.manuscripts.compile_snapshot(&id).await?;
    let data = tokio::task::spawn_blocking(move || -> Result<Vec<u8>, String> {
        use std::io::Write;
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        for (path, bytes) in snapshot.files {
            zip.start_file(path, options).map_err(|e| e.to_string())?;
            zip.write_all(&bytes).map_err(|e| e.to_string())?;
        }
        Ok(zip.finish().map_err(|e| e.to_string())?.into_inner())
    })
    .await
    .map_err(|e| Error::Invalid(e.to_string()))?
    .map_err(Error::Invalid)?;
    value(
        json!({"filename":"manuscript-source.zip","data_base64":base64::engine::general_purpose::STANDARD.encode(data)}),
    )
}
