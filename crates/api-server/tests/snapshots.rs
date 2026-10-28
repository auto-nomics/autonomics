//! Actual server-owned captures and persistent canonical DB integration.
use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode},
};
use bib_base::{
    BibShared,
    query::{LiteratureGateway, LiteratureSource, SourceBatch},
};
use bib_types::{Article, Identifier, StructuredSearch, discovery::*};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use std::sync::Arc;
use tower::ServiceExt;
fn request(method: &str, path: &str, body: Option<Value>) -> Request<Body> {
    let mut builder = Request::builder().method(method).uri(path);
    if body.is_some() {
        builder = builder.header("content-type", "application/json");
    }
    builder
        .body(
            body.map(|value| Body::from(value.to_string()))
                .unwrap_or_else(Body::empty),
        )
        .unwrap()
}
async fn call(app: &Router, method: &str, path: &str, input: Option<Value>) -> (StatusCode, Value) {
    let response = app
        .clone()
        .oneshot(request(method, path, input))
        .await
        .unwrap();
    assert_eq!(response.headers()["cache-control"], "no-store");
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap()
        },
    )
}
fn library(key: &str) -> Value {
    json!({"key":key,"name":"Frozen review","max_items":10,"execution":{"kind":"library","search":{"query":"Study","page_size":2}}})
}
async fn populated() -> BibShared {
    let shared = BibShared::open_in_memory().await.unwrap();
    for id in ["a", "b", "c"] {
        shared
            .bib
            .upsert_article(&Article::new(id, format!("Study {id}")))
            .await
            .unwrap();
    }
    shared
}

#[tokio::test]
async fn capture_and_exports_stay_frozen_after_live_edits_and_idempotent_retry() {
    let shared = populated().await;
    let app = api_server::bib::router(shared.clone());
    let (status, result) = call(
        &app,
        "POST",
        "/search/snapshots",
        Some(library("library.freeze")),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let saved = result["snapshot"].clone();
    assert_eq!(saved["summary"]["coverage"]["captured_items"], 3);
    assert_eq!(saved["summary"]["coverage"]["complete_obtained_set"], true);
    assert_eq!(
        saved["capture"]["data"]["pages"].as_array().unwrap().len(),
        2
    );
    let id = saved["summary"]["id"].as_str().unwrap();
    shared.bib.delete_article("a").await.unwrap();
    shared
        .bib
        .upsert_article(&Article::new("b", "Changed current title"))
        .await
        .unwrap();
    let (status, retried) = call(
        &app,
        "POST",
        "/search/snapshots",
        Some(library("library.freeze")),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(retried["snapshot"], saved);
    for format in ["json", "ris", "bibtex"] {
        let (status, export) = call(
            &app,
            "POST",
            &format!("/search/snapshots/{id}/export"),
            Some(json!({"format":format})),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(export["articles"], 3);
        assert!(export["content"].as_str().unwrap().contains("Study a"));
        assert!(
            !export["content"]
                .as_str()
                .unwrap()
                .contains("Changed current title")
        );
    }
    let mut changed = library("library.freeze");
    changed["name"] = json!("Different action");
    assert_eq!(
        call(&app, "POST", "/search/snapshots", Some(changed))
            .await
            .0,
        StatusCode::CONFLICT
    );
    let (status, read) = call(&app, "GET", &format!("/search/snapshots/{id}"), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(read["snapshot"], saved);
}

#[tokio::test]
async fn server_capture_rejects_forged_results_and_reports_real_item_budget() {
    let shared = populated().await;
    let app = api_server::bib::router(shared);
    let mut bad = library("forged.capture");
    bad["articles"] = json!([{ "id":"fake","title":"Fake result" }]);
    let response = app
        .clone()
        .oneshot(request("POST", "/search/snapshots", Some(bad)))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let mut input = library("bounded.capture");
    input["max_items"] = json!(1);
    let (status, value) = call(&app, "POST", "/search/snapshots", Some(input)).await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(
        value["snapshot"]["summary"]["coverage"]["captured_items"],
        1
    );
    assert_eq!(
        value["snapshot"]["summary"]["coverage"]["complete_obtained_set"],
        false
    );
    assert_eq!(
        value["snapshot"]["capture"]["data"]["termination"],
        "budget"
    );
}

#[tokio::test]
async fn persisted_budget_and_checksum_delete_preserve_data_and_release_capacity() {
    let shared = populated().await;
    let app = api_server::bib::router(shared);
    let limits =
        json!({"max_snapshots":1,"max_snapshot_bytes":8388608,"max_total_bytes":268435456});
    assert_eq!(
        call(
            &app,
            "PUT",
            "/search/snapshots/limits",
            Some(limits.clone())
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        call(&app, "GET", "/search/snapshots/limits", None).await.1,
        limits
    );
    let (_, saved) = call(
        &app,
        "POST",
        "/search/snapshots",
        Some(library("quota.first")),
    )
    .await;
    let id = saved["snapshot"]["summary"]["id"].as_str().unwrap();
    let checksum = saved["snapshot"]["summary"]["checksum"].as_str().unwrap();
    assert_eq!(
        call(
            &app,
            "POST",
            "/search/snapshots",
            Some(library("quota.second"))
        )
        .await
        .0,
        StatusCode::PAYLOAD_TOO_LARGE
    );
    assert_eq!(
        call(
            &app,
            "DELETE",
            &format!("/search/snapshots/{id}?expected_checksum=wrong"),
            None
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    let (_, list) = call(&app, "GET", "/search/snapshots", None).await;
    assert_eq!(list["snapshots"].as_array().unwrap().len(), 1);
    assert_eq!(
        call(
            &app,
            "DELETE",
            &format!("/search/snapshots/{id}?expected_checksum={checksum}"),
            None
        )
        .await
        .1["deleted"],
        true
    );
    assert_eq!(
        call(
            &app,
            "POST",
            "/search/snapshots",
            Some(library("quota.second"))
        )
        .await
        .0,
        StatusCode::CREATED
    );
}

#[tokio::test]
async fn canonical_database_reopen_keeps_snapshots_after_article_deletion() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("library.db");
    let shared = BibShared::open(&path).await.unwrap();
    shared
        .bib
        .upsert_article(&Article::new("a", "Study original"))
        .await
        .unwrap();
    let app = api_server::bib::router(shared.clone());
    let (_, saved) = call(
        &app,
        "POST",
        "/search/snapshots",
        Some(library("persistent.capture")),
    )
    .await;
    assert!(saved["snapshot"].is_object(), "{saved}");
    let id = saved["snapshot"]["summary"]["id"].as_str().unwrap();
    shared.bib.delete_article("a").await.unwrap();
    drop(app);
    drop(shared);
    let reopened = BibShared::open(&path).await.unwrap();
    assert!(reopened.bib.get_article("a").await.unwrap().is_none());
    let app = api_server::bib::router(reopened);
    assert_eq!(
        call(&app, "GET", &format!("/search/snapshots/{id}"), None)
            .await
            .1,
        saved
    );
    // Tables are in the canonical DB, not an untracked sidecar.
    let db = canonical_snapshot_count(&path).await;
    assert_eq!(db, 1);
}
async fn canonical_snapshot_count(path: &std::path::Path) -> usize {
    let store = bib_base::search_snapshots::SnapshotStore::open(path.to_str().unwrap())
        .await
        .unwrap();
    store.list(100, None).await.unwrap().snapshots.len()
}

struct Source {
    failed: bool,
}
#[async_trait::async_trait]
impl LiteratureSource for Source {
    fn name(&self) -> &'static str {
        if self.failed { "failed" } else { "fixture" }
    }
    fn discovery_capabilities(&self) -> SourceCapabilities {
        SourceCapabilities {
            source: self.name().into(),
            searchable: true,
            native_query: false,
            fields: vec![bib_types::SearchField::Any],
            filters: vec![],
            sorts: vec![DiscoverySort::Relevance],
            pagination: SourcePagination::Cursor,
            max_page_size: 100,
            credentials_configured: false,
            authentication: SourceAuthentication::None,
            quota: None,
            notes: vec![],
        }
    }
    fn plan_discovery(
        &self,
        request: &DiscoveryRequest,
    ) -> std::result::Result<SourcePlan, SourceFailure> {
        Ok(SourcePlan {
            source: self.name().into(),
            page_size: request.page_size,
            parameters: Default::default(),
            fingerprint: "snapshot.fixture".into(),
            notices: vec![],
        })
    }
    async fn discovery_page(
        &self,
        _: &SourcePlan,
        _: Option<&str>,
        _: u16,
    ) -> std::result::Result<SourcePage, SourceFailure> {
        if self.failed {
            return Err(SourceFailure::new(
                SourceFailureKind::Upstream,
                "Fixture source failed",
                false,
            ));
        }
        Ok(SourcePage {
            source: self.name().into(),
            articles: vec![
                Article::new("one", "Fixture one"),
                Article::new("two", "Fixture two"),
            ],
            total: Some(2),
            next_cursor: None,
            executed_parameters: [("actual".into(), "original query".into())].into(),
            query_translation: Some("Preserved translation".into()),
            notices: vec![],
        })
    }
    async fn search(&self, _: &StructuredSearch, _: usize) -> bib_base::Result<SourceBatch> {
        unreachable!()
    }
    async fn fetch(&self, _: &Identifier) -> bib_base::Result<Option<Article>> {
        unreachable!()
    }
}
#[tokio::test]
async fn discovery_capture_is_consistent_and_retry_survives_session_removal() {
    let mut shared = populated().await;
    shared.gateway = Arc::new(
        LiteratureGateway::new()
            .with_source(Arc::new(Source { failed: false }))
            .with_source(Arc::new(Source { failed: true })),
    );
    let initial = shared
        .discovery_sessions
        .create(
            shared.gateway.clone(),
            DiscoveryRequest {
                query: "fixture".into(),
                sources: vec!["fixture".into(), "failed".into()],
                ..Default::default()
            },
        )
        .unwrap();
    let session = tokio::time::timeout(std::time::Duration::from_secs(3), async {
        loop {
            let session = shared.discovery_sessions.snapshot(&initial.id).unwrap();
            if session.phase != DiscoverySessionPhase::Running {
                break session;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    let input = json!({"key":"discovery.capture","name":"Discovery record","max_items":1,"execution":{"kind":"discovery","session_id":session.id,"sequence":session.sequence,"order":"first_seen"}});
    let app = api_server::bib::router(shared.clone());
    let mut changed = input.clone();
    changed["execution"]["sequence"] = json!(session.sequence - 1);
    assert_eq!(
        call(&app, "POST", "/search/snapshots", Some(changed))
            .await
            .0,
        StatusCode::CONFLICT
    );
    let (status, saved) = call(&app, "POST", "/search/snapshots", Some(input.clone())).await;
    assert_eq!(status, StatusCode::CREATED, "{saved}");
    assert_eq!(
        saved["snapshot"]["capture"]["data"]["session"]["sequence"],
        saved["snapshot"]["capture"]["data"]["results"]["sequence"]
    );
    assert_eq!(
        saved["snapshot"]["summary"]["coverage"]["captured_items"],
        1
    );
    assert_eq!(
        saved["snapshot"]["summary"]["coverage"]["sources_exhausted"],
        false
    );
    assert_eq!(
        saved["snapshot"]["capture"]["data"]["results"]["candidates"][0]["records"][0]["executed_parameters"]
            ["actual"],
        "original query"
    );
    shared.discovery_sessions.delete(&session.id).unwrap();
    assert_eq!(
        call(&app, "POST", "/search/snapshots", Some(input)).await.1,
        saved
    );
}

#[tokio::test]
async fn hybrid_snapshot_records_actual_model_and_retries_after_model_is_disabled() {
    let digest = "a".repeat(64);
    let blocked = Arc::new(tokio::sync::Notify::new());
    let released = Arc::new(tokio::sync::Notify::new());
    let entered = blocked.clone();
    let release = released.clone();
    let provider=Router::new()
        .route("/api/tags",axum::routing::get(move ||async move {axum::Json(json!({"models":[{"name":format!("autonomics-embedding:{digest}"),"digest":digest},{"name":"fixture:latest","digest":"a".repeat(64)}]}))}))
        .route("/api/embed",axum::routing::post(move |axum::Json(input):axum::Json<Value>|{
            let entered=entered.clone();let release=release.clone();async move {
                if input["input"].as_array().unwrap().iter().any(|value|value.as_str().unwrap().contains("blocked snapshot query")) {entered.notify_one();release.notified().await;}
                axum::Json(json!({"embeddings":vec![vec![1.0,0.0];input["input"].as_array().unwrap().len()]}))
            }
        }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        axum::serve(listener, provider).await.unwrap();
    });
    let shared = populated().await;
    let mut config = bib_base::semantic_service::SemanticConfig {
        enabled: true,
        origin,
        model: "fixture:latest".into(),
        model_digest: "a".repeat(64),
        ..Default::default()
    };
    config.profile.dimensions = 2;
    shared
        .bib
        .configure_semantic_search(config.clone())
        .await
        .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            let status = shared.bib.semantic_search_status().unwrap();
            assert_ne!(
                status.phase,
                bib_base::semantic_service::SemanticPhase::Failed,
                "{status:?}"
            );
            if status.phase == bib_base::semantic_service::SemanticPhase::Ready {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let app = api_server::bib::router(shared.clone());
    let mut input = library("hybrid.capture");
    input["execution"]["search"] =
        json!({"query":"","mode":"hybrid","semantic_query":"immune research","page_size":2});
    let (status, saved) = call(&app, "POST", "/search/snapshots", Some(input.clone())).await;
    assert_eq!(status, StatusCode::CREATED, "{saved}");
    let capture = &saved["snapshot"]["capture"];
    assert_eq!(capture["runtime"]["model"]["digest"], "a".repeat(64));
    let model = &capture["runtime"]["model"];
    for page in capture["data"]["pages"].as_array().unwrap() {
        assert_eq!(
            model["semantic_generation"],
            page["response"]["retrieval"]["semantic_generation"]
        );
        assert_eq!(
            model["configuration_generation"],
            page["response"]["retrieval"]["configuration_generation"]
        );
    }
    assert_eq!(
        saved["snapshot"]["summary"]["coverage"]["result_basis"],
        "candidate_pool"
    );
    let mut cancelled = input.clone();
    cancelled["key"] = json!("hybrid.cancelled");
    cancelled["execution"]["search"]["semantic_query"] = json!("blocked snapshot query");
    let mut capture_request = request("POST", "/search/snapshots", Some(cancelled));
    capture_request.headers_mut().insert(
        "x-search-request-id",
        "snapshot.cancel-active".parse().unwrap(),
    );
    let pending = tokio::spawn(app.clone().oneshot(capture_request));
    tokio::time::timeout(std::time::Duration::from_secs(3), blocked.notified())
        .await
        .unwrap();
    let response = app
        .clone()
        .oneshot(request(
            "POST",
            "/search/cancel",
            Some(json!({"request_id":"snapshot.cancel-active"})),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    let response = tokio::time::timeout(std::time::Duration::from_secs(1), pending)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(response.status().as_u16(), 499);
    released.notify_one();
    tokio::time::sleep(std::time::Duration::from_millis(30)).await;
    assert_eq!(
        shared
            .search_snapshots
            .list(100, None)
            .await
            .unwrap()
            .snapshots
            .len(),
        1
    );
    config.enabled = false;
    shared.bib.configure_semantic_search(config).await.unwrap();
    server.abort();
    let (status, retry) = call(&app, "POST", "/search/snapshots", Some(input)).await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(retry, saved);
}

#[tokio::test]
async fn cancelled_before_capture_returns_499_and_publishes_no_snapshot() {
    let shared = populated().await;
    shared
        .search_cancellations
        .cancel("snapshot.cancel-before")
        .unwrap();
    let app = api_server::bib::router(shared);
    let mut query = request(
        "POST",
        "/search/snapshots",
        Some(library("cancelled.capture")),
    );
    query.headers_mut().insert(
        "x-search-request-id",
        "snapshot.cancel-before".parse().unwrap(),
    );
    let response = app.clone().oneshot(query).await.unwrap();
    assert_eq!(response.status().as_u16(), 499);
    let (_, list) = call(&app, "GET", "/search/snapshots", None).await;
    assert!(list["snapshots"].as_array().unwrap().is_empty());
}
