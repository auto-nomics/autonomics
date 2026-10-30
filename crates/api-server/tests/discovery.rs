use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode},
};
use bib_base::{
    BibShared,
    discovery_sessions::{DiscoveryLimits, DiscoverySessions},
    query::{LiteratureGateway, LiteratureSource, SourceBatch},
};
use bib_types::{Article, Identifier, StructuredSearch, discovery::*};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use tokio::sync::Notify;
use tower::ServiceExt;

struct Source {
    name: &'static str,
    calls: AtomicUsize,
    gate: Arc<Notify>,
}
#[async_trait::async_trait]
impl LiteratureSource for Source {
    fn name(&self) -> &'static str {
        self.name
    }
    fn discovery_capabilities(&self) -> SourceCapabilities {
        SourceCapabilities {
            source: self.name.into(),
            searchable: true,
            native_query: false,
            fields: vec![bib_types::SearchField::Any],
            filters: Vec::new(),
            sorts: vec![DiscoverySort::Relevance],
            pagination: SourcePagination::Cursor,
            max_page_size: 100,
            credentials_configured: false,
            authentication: SourceAuthentication::None,
            quota: None,
            notes: Vec::new(),
        }
    }
    fn plan_discovery(&self, request: &DiscoveryRequest) -> Result<SourcePlan, SourceFailure> {
        Ok(SourcePlan {
            source: self.name.into(),
            page_size: request.page_size,
            parameters: Default::default(),
            fingerprint: "planned-fixture".into(),
            notices: Vec::new(),
        })
    }
    async fn discovery_page(
        &self,
        _: &SourcePlan,
        cursor: Option<&str>,
        _: u16,
    ) -> Result<SourcePage, SourceFailure> {
        let call = self.calls.fetch_add(1, Ordering::AcqRel);
        if self.name == "slow" && call == 0 {
            self.gate.notified().await;
        }
        let id = if cursor.is_some() { "two" } else { "one" };
        let mut article = Article::new(
            format!("{}-{id}", self.name),
            format!("{} paper {id}", self.name),
        );
        article
            .identifiers
            .push(Identifier::doi(format!("10.1000/{id}")));
        Ok(SourcePage {
            source: self.name.into(),
            articles: vec![article],
            total: Some(if self.name == "fast" { 3 } else { 2 }),
            next_cursor: if self.name == "fast" && cursor.is_none() {
                Some("page-two".into())
            } else {
                None
            },
            executed_parameters: [("term".into(), "gene".into())].into(),
            query_translation: Some("native query details".into()),
            notices: Vec::new(),
        })
    }
    async fn search(&self, _: &StructuredSearch, _: usize) -> bib_base::Result<SourceBatch> {
        unreachable!()
    }
    async fn fetch(&self, _: &Identifier) -> bib_base::Result<Option<Article>> {
        unreachable!()
    }
}
async fn call(
    app: &Router,
    method: &str,
    path: &str,
    body: Option<Value>,
    auth: bool,
) -> (StatusCode, Value) {
    call_with_id(app, method, path, body, auth, None).await
}
async fn call_with_id(
    app: &Router,
    method: &str,
    path: &str,
    body: Option<Value>,
    auth: bool,
    client_id: Option<&str>,
) -> (StatusCode, Value) {
    let mut request = Request::builder().method(method).uri(path);
    if let Some(id) = client_id {
        request = request.header("x-discovery-session-id", id);
    }
    if auth {
        request = request.header("authorization", "Bearer fixture-token");
    }
    if body.is_some() {
        request = request.header("content-type", "application/json");
    }
    let response = app
        .clone()
        .oneshot(
            request
                .body(
                    body.map(|body| Body::from(body.to_string()))
                        .unwrap_or_else(Body::empty),
                )
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    if status != StatusCode::UNAUTHORIZED {
        assert_eq!(response.headers().get("cache-control").unwrap(), "no-store");
    }
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes)
                .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into_owned()))
        },
    )
}

#[tokio::test]
async fn client_id_header_makes_creation_idempotent_and_early_close_authoritative() {
    let mut shared = BibShared::open_in_memory().await.unwrap();
    let source = Arc::new(Source {
        name: "fast",
        calls: AtomicUsize::new(0),
        gate: Arc::new(Notify::new()),
    });
    shared.gateway = Arc::new(LiteratureGateway::new().with_source(source.clone()));
    let app = api_server::api_router_with_auth(shared, None, Some("fixture-token".into()));
    let id = uuid::Uuid::new_v4().to_string();
    let base = format!("/api/v1/bib/search/sessions/{id}");
    let body = json!({"query":"gene","sources":["fast"],"page_size":1});
    assert_eq!(
        call(&app, "DELETE", &base, None, true).await.0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        call_with_id(
            &app,
            "POST",
            "/api/v1/bib/search/sessions",
            Some(body.clone()),
            true,
            Some(&id)
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    assert_eq!(source.calls.load(Ordering::Acquire), 0);
    let id = uuid::Uuid::new_v4().to_string();
    let created = call_with_id(
        &app,
        "POST",
        "/api/v1/bib/search/sessions",
        Some(body.clone()),
        true,
        Some(&id),
    )
    .await;
    assert_eq!(created.0, StatusCode::CREATED);
    assert_eq!(created.1["session"]["id"], id);
    let duplicate = call_with_id(
        &app,
        "POST",
        "/api/v1/bib/search/sessions",
        Some(body.clone()),
        true,
        Some(&id),
    )
    .await;
    assert_eq!(duplicate.1["session"]["id"], id);
    wait(&app, &format!("/api/v1/bib/search/sessions/{id}"), |body| {
        body["session"]["phase"] == "idle"
    })
    .await;
    assert_eq!(source.calls.load(Ordering::Acquire), 1);
    assert_eq!(
        call_with_id(
            &app,
            "POST",
            "/api/v1/bib/search/sessions",
            Some(body),
            true,
            Some("invalid")
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
}
async fn wait(app: &Router, path: &str, condition: impl Fn(&Value) -> bool) -> Value {
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        loop {
            let (status, value) = call(app, "GET", path, None, true).await;
            assert_eq!(status, StatusCode::OK, "{value}");
            if condition(&value) {
                break value;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap()
}

#[tokio::test]
async fn imports_check_candidate_revision_preserve_curated_metadata_and_outlive_sessions() {
    let mut shared = BibShared::open_in_memory().await.unwrap();
    let bib = shared.bib.clone();
    let mut curated = Article::new("curated", "Curated local title");
    curated.identifiers.push(Identifier::doi("10.1000/one"));
    bib.upsert_article(&curated).await.unwrap();
    let gate = Arc::new(Notify::new());
    let fast = Arc::new(Source {
        name: "fast",
        calls: AtomicUsize::new(0),
        gate: gate.clone(),
    });
    let slow = Arc::new(Source {
        name: "slow",
        calls: AtomicUsize::new(0),
        gate: gate.clone(),
    });
    shared.gateway = Arc::new(
        LiteratureGateway::new()
            .with_source(fast)
            .with_source(slow.clone()),
    );
    let app = api_server::api_router_with_auth(shared, None, Some("fixture-token".into()));
    let created = call(
        &app,
        "POST",
        "/api/v1/bib/search/sessions",
        Some(json!({"query":"gene","sources":["fast","slow"]})),
        true,
    )
    .await;
    assert_eq!(created.0, StatusCode::CREATED);
    let base = format!(
        "/api/v1/bib/search/sessions/{}",
        created.1["session"]["id"].as_str().unwrap()
    );
    wait(&app, &base, |body| {
        body["session"]["merged_candidates"] == 1 && slow.calls.load(Ordering::Acquire) == 1
    })
    .await;
    let initial = call(&app, "GET", &format!("{base}/results"), None, true)
        .await
        .1;
    let candidate = &initial["candidates"][0];
    let stale = json!({"selections":[{"candidate_id":candidate["id"],"expected_fingerprint":candidate["fingerprint"]}]});
    let preview = call(
        &app,
        "POST",
        &format!("{base}/import/preview"),
        Some(stale.clone()),
        true,
    )
    .await;
    assert_eq!(preview.0, StatusCode::OK, "{}", preview.1);
    assert_eq!(
        preview.1["preview"]["items"][0]["existing"][0]["title"],
        "Curated local title"
    );
    assert_eq!(bib.article_count().await.unwrap(), 1);
    gate.notify_waiters();
    wait(&app, &base, |body| body["session"]["phase"] == "idle").await;
    for endpoint in ["import/preview", "import"] {
        assert_eq!(
            call(
                &app,
                "POST",
                &format!("{base}/{endpoint}"),
                Some(stale.clone()),
                true
            )
            .await
            .0,
            StatusCode::CONFLICT
        );
    }
    let current = call(&app, "GET", &format!("{base}/results"), None, true)
        .await
        .1;
    let candidate = &current["candidates"][0];
    let record_index = candidate["records"]
        .as_array()
        .unwrap()
        .iter()
        .position(|record| record["source"] == "slow")
        .unwrap();
    let selection = json!({"selections":[{"candidate_id":candidate["id"],"expected_fingerprint":candidate["fingerprint"],"record_index":record_index}]});
    for attempt in 0..2 {
        let imported = call(
            &app,
            "POST",
            &format!("{base}/import"),
            Some(selection.clone()),
            true,
        )
        .await;
        assert_eq!(imported.0, StatusCode::OK, "{}", imported.1);
        assert_eq!(imported.1["items"][0]["status"], "already_present");
        assert_eq!(
            imported.1["items"][0]["article"]["title"],
            "Curated local title"
        );
        if attempt == 1 {
            assert_eq!(imported.1["items"][0]["changed"], false);
        }
    }
    assert_eq!(bib.article_count().await.unwrap(), 1);
    assert_eq!(
        call(&app, "DELETE", &base, None, true).await.0,
        StatusCode::NO_CONTENT
    );
    let provenance = call(
        &app,
        "GET",
        "/api/v1/bib/articles/curated/discovery-provenance",
        None,
        true,
    )
    .await;
    assert_eq!(provenance.0, StatusCode::OK);
    assert_eq!(provenance.1["total"], 1);
    let stored = &provenance.1["entries"][0]["provenance"];
    assert_eq!(stored["selected"]["source"], "slow");
    assert_eq!(stored["selected"]["executed_parameters"]["term"], "gene");
    assert_eq!(
        stored["selected"]["query_translation"],
        "native query details"
    );
    assert_eq!(stored["records"].as_array().unwrap().len(), 2);
    assert_eq!(stored["request"]["query"], "gene");
    assert_eq!(
        bib.get_article("curated").await.unwrap().unwrap().title,
        "Curated local title"
    );
}

#[tokio::test]
async fn fulltext_jobs_reuse_durable_attachments_and_survive_session_deletion() {
    let mut shared = BibShared::open_in_memory()
        .await
        .unwrap()
        .with_file_storage(Arc::new(vfs::OpendalFileStorage::new_temp()));
    let mut article = Article::new("existing-pdf", "Existing PDF");
    article.identifiers.push(Identifier::doi("10.1000/one"));
    shared.bib.upsert_article(&article).await.unwrap();
    let pdf = bib_base::managed_fulltext::store_pdf(
        &shared,
        &article.id,
        bib_base::pmc_pdf::PmcPdf {
            pmcid: "PMC42".into(),
            version: 1,
            source_url: "https://example.org/PMC42.1.pdf".into(),
            bytes: b"%PDF-1.7\nfixture\n%%EOF".to_vec(),
        },
    )
    .await
    .unwrap();
    let bib = shared.bib.clone();
    shared.gateway = Arc::new(LiteratureGateway::new().with_source(Arc::new(Source {
        name: "fast",
        calls: AtomicUsize::new(0),
        gate: Arc::new(Notify::new()),
    })));
    let app = api_server::api_router_with_auth(shared, None, Some("fixture-token".into()));
    let created = call(
        &app,
        "POST",
        "/api/v1/bib/search/sessions",
        Some(json!({"query":"gene","sources":["fast"]})),
        true,
    )
    .await;
    let base = format!(
        "/api/v1/bib/search/sessions/{}",
        created.1["session"]["id"].as_str().unwrap()
    );
    wait(&app, &base, |value| value["session"]["phase"] == "idle").await;
    let page = call(&app, "GET", &format!("{base}/results"), None, true)
        .await
        .1;
    let candidate = &page["candidates"][0];
    let imported = call(&app, "POST", &format!("{base}/import"), Some(json!({"selections":[{"candidate_id":candidate["id"],"expected_fingerprint":candidate["fingerprint"]}],"fetch_fulltext":true})), true).await;
    assert_eq!(imported.0, StatusCode::OK, "{}", imported.1);
    assert_eq!(imported.1["items"][0]["status"], "already_present");
    let job = format!(
        "/api/v1/bib/search/acquisitions/{}",
        imported.1["acquisition"]["id"].as_str().unwrap()
    );
    assert_eq!(
        call(&app, "GET", &job, None, false).await.0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        call(&app, "DELETE", &base, None, true).await.0,
        StatusCode::NO_CONTENT
    );
    let complete = wait(&app, &job, |value| {
        value["acquisition"]["completed_at"].is_string()
    })
    .await;
    assert_eq!(complete["acquisition"]["items"][0]["status"], "available");
    assert_eq!(bib.zotero_attachments(&article.id).await.unwrap().len(), 1);
    assert_eq!(
        bib.get_fulltext(&article.id)
            .await
            .unwrap()
            .unwrap()
            .file_path,
        pdf.file_path
    );
    assert_eq!(
        call(&app, "POST", &format!("{job}/cancel"), None, true)
            .await
            .0,
        StatusCode::OK
    );
    assert_eq!(
        call(
            &app,
            "GET",
            "/api/v1/bib/search/acquisitions/unknown",
            None,
            true
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn fulltext_preflight_rejects_missing_storage_and_full_queue_before_metadata_writes() {
    let mut shared = BibShared::open_in_memory().await.unwrap();
    shared.gateway = Arc::new(LiteratureGateway::new().with_source(Arc::new(Source {
        name: "fast",
        calls: AtomicUsize::new(0),
        gate: Arc::new(Notify::new()),
    })));
    let bib = shared.bib.clone();
    let acquisitions = shared.discovery_acquisitions.clone();
    let app = api_server::api_router_with_auth(shared.clone(), None, Some("fixture-token".into()));
    let created = call(
        &app,
        "POST",
        "/api/v1/bib/search/sessions",
        Some(json!({"query":"gene","sources":["fast"]})),
        true,
    )
    .await;
    let base = format!(
        "/api/v1/bib/search/sessions/{}",
        created.1["session"]["id"].as_str().unwrap()
    );
    wait(&app, &base, |value| value["session"]["phase"] == "idle").await;
    let page = call(&app, "GET", &format!("{base}/results"), None, true)
        .await
        .1;
    let candidate = &page["candidates"][0];
    let request = json!({"selections":[{"candidate_id":candidate["id"],"expected_fingerprint":candidate["fingerprint"]}],"fetch_fulltext":true});
    assert_eq!(
        call(
            &app,
            "POST",
            &format!("{base}/import"),
            Some(request.clone()),
            true
        )
        .await
        .0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    assert_eq!(bib.article_count().await.unwrap(), 0);
    let app = api_server::api_router_with_auth(
        shared.with_file_storage(Arc::new(vfs::OpendalFileStorage::new_temp())),
        None,
        Some("fixture-token".into()),
    );
    let reservations: Vec<_> = (0..16).map(|_| acquisitions.reserve().unwrap()).collect();
    assert_eq!(
        call(
            &app,
            "POST",
            &format!("{base}/import"),
            Some(request.clone()),
            true
        )
        .await
        .0,
        StatusCode::TOO_MANY_REQUESTS
    );
    assert_eq!(bib.article_count().await.unwrap(), 0);
    drop(reservations);
    let mut metadata_only = request;
    metadata_only["fetch_fulltext"] = json!(false);
    let imported = call(
        &app,
        "POST",
        &format!("{base}/import"),
        Some(metadata_only),
        true,
    )
    .await;
    assert_eq!(imported.0, StatusCode::OK);
    assert_eq!(imported.1["imported"], 1);
    assert!(imported.1["acquisition"].is_null());
    assert_eq!(bib.article_count().await.unwrap(), 1);
}

#[tokio::test]
async fn authenticated_discovery_routes_deliver_partial_batches_retry_page_and_retain_provenance() {
    let mut shared = BibShared::open_in_memory().await.unwrap();
    let gate = Arc::new(Notify::new());
    let fast = Arc::new(Source {
        name: "fast",
        calls: AtomicUsize::new(0),
        gate: gate.clone(),
    });
    let slow = Arc::new(Source {
        name: "slow",
        calls: AtomicUsize::new(0),
        gate: gate.clone(),
    });
    shared.gateway = Arc::new(
        LiteratureGateway::new()
            .with_source(fast.clone())
            .with_source(slow.clone()),
    );
    shared.discovery_sessions = DiscoverySessions::new(DiscoveryLimits {
        max_sessions: 1,
        retained_events: 4,
        ..Default::default()
    })
    .unwrap();
    let app = api_server::api_router_with_auth(shared, None, Some("fixture-token".into()));
    assert_eq!(
        call(&app, "GET", "/api/v1/bib/search/sources", None, false)
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    let caps = call(&app, "GET", "/api/v1/bib/search/sources", None, true).await;
    assert_eq!(caps.0, StatusCode::OK);
    assert_eq!(caps.1["sources"].as_array().unwrap().len(), 2);
    let body = json!({"query":"gene","sources":["fast","slow"],"page_size":1});
    let created = call(
        &app,
        "POST",
        "/api/v1/bib/search/sessions",
        Some(body.clone()),
        true,
    )
    .await;
    assert_eq!(created.0, StatusCode::CREATED, "{}", created.1);
    let id = created.1["session"]["id"].as_str().unwrap();
    let base = format!("/api/v1/bib/search/sessions/{id}");
    assert_eq!(
        call(
            &app,
            "POST",
            "/api/v1/bib/search/sessions",
            Some(body),
            true
        )
        .await
        .0,
        StatusCode::TOO_MANY_REQUESTS
    );
    let partial = wait(&app, &base, |body| {
        body["session"]["merged_candidates"] == 1 && slow.calls.load(Ordering::Acquire) == 1
    })
    .await;
    assert_eq!(partial["session"]["phase"], "running");
    let page = call(&app, "GET", &format!("{base}/results"), None, true).await;
    assert_eq!(page.0, StatusCode::OK);
    assert_eq!(page.1["obtained_candidates"], 1);
    let candidate = page.1["candidates"][0]["id"].as_str().unwrap().to_owned();
    assert_eq!(
        call(
            &app,
            "GET",
            &format!("{base}/results?sequence=999999"),
            None,
            true
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        call(
            &app,
            "GET",
            &format!("{base}/events?wait_ms=30000"),
            None,
            true
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        call(
            &app,
            "POST",
            &format!("{base}/cancel"),
            Some(json!({"source":"slow"})),
            true
        )
        .await
        .0,
        StatusCode::OK
    );
    gate.notify_waiters();
    assert_eq!(
        call(
            &app,
            "POST",
            &format!("{base}/sources/slow/retry"),
            None,
            true
        )
        .await
        .0,
        StatusCode::OK
    );
    let complete = wait(&app, &base, |body| body["session"]["phase"] == "idle").await;
    assert_eq!(complete["session"]["sources"][0]["total"], 3);
    assert_eq!(complete["session"]["sources"][1]["total"], 2);
    assert_eq!(complete["session"]["merged_candidates"], 1);
    let detail = call(
        &app,
        "GET",
        &format!("{base}/candidates/{candidate}"),
        None,
        true,
    )
    .await;
    assert_eq!(
        detail.1["candidate"]["records"].as_array().unwrap().len(),
        2
    );
    assert!(
        detail.1["candidate"]["conflicts"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v["field"] == "title")
    );
    assert_eq!(fast.calls.load(Ordering::Acquire), 1);
    assert_eq!(slow.calls.load(Ordering::Acquire), 2);
    assert_eq!(
        call(
            &app,
            "POST",
            &format!("{base}/sources/fast/next"),
            None,
            true
        )
        .await
        .0,
        StatusCode::OK
    );
    let done = wait(&app, &base, |body| body["session"]["phase"] == "idle").await;
    assert_eq!(done["session"]["merged_candidates"], 2);
    let events = call(&app, "GET", &format!("{base}/events?after=0"), None, true).await;
    assert_eq!(events.1["reset_required"], true);
    let sequence = done["session"]["sequence"].as_u64().unwrap();
    let events = call(
        &app,
        "GET",
        &format!("{base}/events?after={sequence}"),
        None,
        true,
    )
    .await;
    assert_eq!(events.1["events"], json!([]));
    assert_eq!(
        call(&app, "DELETE", &base, None, true).await.0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        call(&app, "GET", &base, None, true).await.0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        call(
            &app,
            "POST",
            "/api/v1/bib/search/sessions",
            Some(json!({"query":"gene","sources":["unknown"]})),
            true
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
}
