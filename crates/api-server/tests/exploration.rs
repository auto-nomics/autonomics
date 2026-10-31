//! P5 HTTP route qualification against a local provider fixture.
use axum::{
    Router,
    body::Body,
    extract::Query,
    http::{Request, StatusCode},
    response::IntoResponse,
    routing::get,
};
use bib_base::BibShared;
use http_body_util::BodyExt;
use serde_json::{Value, json};
use std::{collections::HashMap, sync::Arc};
use tower::ServiceExt;

/// 每个路由构建配一个独立内存 wiki(路由状态需要真实句柄)。
async fn wiki_handle() -> std::sync::Arc<wiki::service::WikiHandle> {
    let storage = std::sync::Arc::new(
        wiki::storage::turso_wiki_storage::TursoWikiStorage::open_in_memory()
            .await
            .unwrap()
            .with_blob_store(wiki::storage::blob::MemoryBlobStore::new()),
    );
    std::sync::Arc::new(wiki::service::start(storage))
}
fn request(body: Value, id: Option<&str>) -> Request<Body> {
    let mut builder =
        Request::post("/api/v1/bib/search/explore").header("content-type", "application/json");
    if let Some(id) = id {
        builder = builder.header("x-search-request-id", id);
    }
    builder.body(Body::from(body.to_string())).unwrap()
}
fn input(kind: &str) -> Value {
    json!({"seed":"a".repeat(40),"kind":{"kind":kind},"page_size":2,"cursor":null,"deadline_ms":3000})
}
async fn body(response: axum::response::Response) -> Value {
    serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap()
}
async fn app(provider: Router) -> (Router, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        axum::serve(listener, provider).await.unwrap();
    });
    let mut shared = BibShared::open_in_memory().await.unwrap();
    shared.s2 = Arc::new(
        shared
            .s2
            .as_ref()
            .clone()
            .with_graph_base_url(&origin)
            .with_reco_base_url(&origin)
            .with_max_retries(0),
    );
    (api_server::api_router(shared, wiki_handle().await), task)
}

#[tokio::test]
async fn graph_route_preserves_direction_provenance_and_original_window() {
    let provider=Router::new().route("/paper/{id}/references",get(|Query(params):Query<HashMap<String,String>>|async move {
        assert_eq!(params["limit"],"2");assert_eq!(params["offset"],"0");assert!(params["fields"].contains("contexts"));
        axum::Json(json!({"offset":0,"next":2,"data":[{"citedPaper":{"paperId":"b".repeat(40),"title":"Referenced paper"}},{"citedPaper":{"paperId":null,"title":"Unresolved"}}]}))
    }));
    let (app, server) = app(provider).await;
    let response = app
        .oneshot(request(input("references"), Some("p5.graph-route")))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["cache-control"], "no-store");
    let page = body(response).await;
    assert_eq!(page["items"].as_array().unwrap().len(), 2);
    assert_eq!(page["unresolved_records"], 1);
    assert_eq!(
        page["items"][0]["citation"]["citing_paper_id"],
        "a".repeat(40)
    );
    assert_eq!(
        page["items"][0]["citation"]["cited_paper_id"],
        "b".repeat(40)
    );
    assert!(page["next_cursor"].is_string());
    assert_eq!(page["source"], "semantic_scholar");
    assert_eq!(page["upstream_snapshot"], false);
    server.abort();
}

#[tokio::test]
async fn recommendation_route_uses_explicit_pool_and_never_returns_citation_edges() {
    let provider=Router::new().route("/papers/forpaper/{id}",get(|Query(params):Query<HashMap<String,String>>|async move {
        assert_eq!(params["from"],"all-cs");
        axum::Json(json!({"recommendedPapers":[{"paperId":"b".repeat(40),"title":"Recommended paper"}]}))
    }));
    let (app, server) = app(provider).await;
    let mut query = input("similar");
    query["kind"]["pool"] = json!("all-cs");
    let response = app.oneshot(request(query, None)).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let page = body(response).await;
    assert!(page["items"][0]["citation"].is_null());
    assert!(page["next_cursor"].is_null());
    assert_eq!(page["kind"]["pool"], "all-cs");
    server.abort();
}

#[tokio::test]
async fn route_rate_limits_preserve_retry_after_and_hide_provider_body() {
    let provider = Router::new().route(
        "/paper/{id}/citations",
        get(|| async {
            (
                StatusCode::TOO_MANY_REQUESTS,
                [("retry-after", "17")],
                "private upstream body",
            )
                .into_response()
        }),
    );
    let (app, server) = app(provider).await;
    let response = app
        .oneshot(request(input("citations"), None))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(response.headers()["retry-after"], "17");
    let error = body(response).await;
    assert_eq!(error["failure"]["kind"], "rate_limited");
    assert_eq!(error["failure"]["retry_after_seconds"], 17);
    assert!(!error.to_string().contains("private upstream body"));
    server.abort();
}

#[tokio::test]
async fn route_cancellation_stops_a_waiting_provider_request() {
    let entered = Arc::new(tokio::sync::Notify::new());
    let started = entered.clone();
    let provider = Router::new().route(
        "/paper/{id}/citations",
        get(move || {
            let entered = entered.clone();
            async move {
                entered.notify_one();
                std::future::pending::<axum::Json<Value>>().await
            }
        }),
    );
    let (app, server) = app(provider).await;
    let search = tokio::spawn(
        app.clone()
            .oneshot(request(input("citations"), Some("p5.cancel-route"))),
    );
    tokio::time::timeout(std::time::Duration::from_secs(3), started.notified())
        .await
        .unwrap();
    let cancel = Request::post("/api/v1/bib/search/cancel")
        .header("content-type", "application/json")
        .body(Body::from(
            json!({"request_id":"p5.cancel-route"}).to_string(),
        ))
        .unwrap();
    assert_eq!(
        app.oneshot(cancel).await.unwrap().status(),
        StatusCode::ACCEPTED
    );
    let response = tokio::time::timeout(std::time::Duration::from_secs(1), search)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(response.status().as_u16(), 499);
    server.abort();
}

#[tokio::test]
async fn route_invalid_requests_never_become_successful_empty_lists() {
    let (app, server) = app(Router::new()).await;
    let mut bad = input("references");
    bad["page_size"] = json!(0);
    assert_eq!(
        app.clone()
            .oneshot(request(bad, None))
            .await
            .unwrap()
            .status(),
        StatusCode::BAD_REQUEST
    );
    let mut bad = input("references");
    bad["cursor"] = json!("invalid");
    assert_eq!(
        app.clone()
            .oneshot(request(bad, None))
            .await
            .unwrap()
            .status(),
        StatusCode::CONFLICT
    );
    let mut bad = input("similar");
    bad["kind"]["pool"] = json!("unbounded-all");
    assert_eq!(
        app.oneshot(request(bad, None)).await.unwrap().status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );
    server.abort();
}

#[tokio::test]
async fn graph_selection_uses_fresh_exact_discovery_and_preserves_curated_library_metadata() {
    let provider=Router::new()
        .route("/paper/{id}/references",get(||async {axum::Json(json!({"offset":0,"data":[{"citedPaper":{"paperId":"b".repeat(40),"title":"Old graph metadata","externalIds":{"DOI":"10.1234/curated"}}}]}))}))
        .route("/paper/{id}",get(||async {axum::Json(json!({"paperId":"b".repeat(40),"title":"Fresh provider metadata","externalIds":{"DOI":"10.1234/curated"}}))}));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        axum::serve(listener, provider).await.unwrap();
    });
    let mut shared = BibShared::open_in_memory().await.unwrap();
    shared.s2 = Arc::new(
        shared
            .s2
            .as_ref()
            .clone()
            .with_graph_base_url(origin)
            .with_max_retries(0),
    );
    shared.gateway = Arc::new(
        bib_base::query::LiteratureGateway::new()
            .with_source(Arc::new(bib_base::query::S2Source::new(shared.s2.clone()))),
    );
    let mut curated = bib_types::Article::new("manual", "User curated title");
    curated
        .identifiers
        .push(bib_types::Identifier::doi("10.1234/curated"));
    curated.keywords = vec!["manual-tag".into()];
    shared.bib.upsert_article(&curated).await.unwrap();
    let app = api_server::api_router(shared.clone(), wiki_handle().await);
    let graph = body(
        app.clone()
            .oneshot(request(input("references"), None))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(graph["items"][0]["article"]["title"], "Old graph metadata");
    let lookup = json!({"sources":["semantic_scholar"],"native":{"source":"semantic_scholar","parameters":{"endpoint":"lookup","paper_id":"b".repeat(40)}},"page_size":1});
    let response = app
        .clone()
        .oneshot(
            Request::post("/api/v1/bib/search/sessions")
                .header("content-type", "application/json")
                .body(Body::from(lookup.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
    let created = body(response).await;
    let id = created["session"]["id"].as_str().unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let value = body(
                app.clone()
                    .oneshot(
                        Request::get(format!("/api/v1/bib/search/sessions/{id}"))
                            .body(Body::empty())
                            .unwrap(),
                    )
                    .await
                    .unwrap(),
            )
            .await;
            if value["session"]["phase"] != "running" {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let page = body(
        app.clone()
            .oneshot(
                Request::get(format!("/api/v1/bib/search/sessions/{id}/results"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    let candidate = &page["candidates"][0];
    assert_eq!(candidate["article"]["title"], "Fresh provider metadata");
    let import = json!({"library_id":1,"fetch_fulltext":false,"selections":[{"candidate_id":candidate["id"],"expected_fingerprint":candidate["fingerprint"],"record_index":0}]});
    let response = app
        .clone()
        .oneshot(
            Request::post(format!("/api/v1/bib/search/sessions/{id}/import/preview"))
                .header("content-type", "application/json")
                .body(Body::from(import.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let preview = body(response).await;
    assert_eq!(
        preview["preview"]["items"][0]["existing"][0]["title"],
        "User curated title"
    );
    let response = app
        .oneshot(
            Request::post(format!("/api/v1/bib/search/sessions/{id}/import"))
                .header("content-type", "application/json")
                .body(Body::from(import.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let imported = body(response).await;
    assert_eq!(imported["failed"], 0);
    assert_eq!(shared.bib.article_count().await.unwrap(), 1);
    let retained = shared.bib.get_article("manual").await.unwrap().unwrap();
    assert_eq!(retained.title, "User curated title");
    assert_eq!(retained.keywords, vec!["manual-tag"]);
    server.abort();
}
