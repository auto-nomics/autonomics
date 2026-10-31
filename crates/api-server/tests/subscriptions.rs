//! Authenticated subscription HTTP routes with injected source I/O.
use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode},
};
use bib_base::{
    BibShared,
    query::{LiteratureGateway, LiteratureSource, SourceBatch},
    subscriptions::*,
};
use bib_types::{Article, Identifier, StructuredSearch, discovery::*};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};
use tower::ServiceExt;
struct Source {
    calls: AtomicUsize,
    hold: AtomicBool,
}
#[async_trait::async_trait]
impl LiteratureSource for Source {
    fn name(&self) -> &'static str {
        "openalex"
    }
    async fn search(&self, _: &StructuredSearch, _: usize) -> bib_base::Result<SourceBatch> {
        unreachable!()
    }
    async fn fetch(&self, _: &Identifier) -> bib_base::Result<Option<Article>> {
        unreachable!()
    }
    async fn discovery_page(
        &self,
        plan: &SourcePlan,
        _: Option<&str>,
        _: u16,
    ) -> std::result::Result<SourcePage, SourceFailure> {
        let call = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
        if self.hold.load(Ordering::SeqCst) {
            return std::future::pending().await;
        }
        let mut article = Article::new(format!("source-{call}"), "Immune source fixture");
        article
            .identifiers
            .push(Identifier::doi(format!("10.1000/{call}")));
        Ok(SourcePage {
            source: "openalex".into(),
            articles: vec![article],
            total: Some(1),
            next_cursor: None,
            executed_parameters: plan.parameters.clone(),
            query_translation: None,
            notices: Vec::new(),
        })
    }
}
async fn setup(hold: bool) -> (BibShared, Arc<Source>, Router) {
    let mut shared = BibShared::open_in_memory().await.unwrap();
    let source = Arc::new(Source {
        calls: AtomicUsize::new(0),
        hold: AtomicBool::new(hold),
    });
    shared.gateway = Arc::new(LiteratureGateway::new().with_source(source.clone()));
    let app = api_server::api_router_with_auth(
        shared.clone(),
        None,
        Some("subscription-test-token".into()),
    );
    (shared, source, app)
}
fn spec() -> Value {
    json!({"name":"Immune watch","request":{"query":"immune","sources":["openalex"]}})
}
async fn call(app: &Router, method: &str, path: &str, value: Option<Value>) -> (StatusCode, Value) {
    let mut request = Request::builder()
        .method(method)
        .uri(format!("/api/v1/bib{path}"))
        .header("authorization", "Bearer subscription-test-token");
    if value.is_some() {
        request = request.header("content-type", "application/json");
    }
    let response = app
        .clone()
        .oneshot(
            request
                .body(
                    value
                        .map(|v| Body::from(v.to_string()))
                        .unwrap_or_else(Body::empty),
                )
                .unwrap(),
        )
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
            serde_json::from_slice(&bytes)
                .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into_owned()))
        },
    )
}
async fn create(app: &Router) -> String {
    let (status, result) = call(app, "POST", "/subscriptions", Some(spec())).await;
    assert_eq!(status, StatusCode::CREATED);
    result["rule"]["id"].as_str().unwrap().into()
}
async fn finished(shared: &BibShared, id: &str) -> SubscriptionRun {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let run = shared.subscriptions.run(id).await.unwrap();
            if run.state != SubscriptionRunState::Running {
                return run;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap()
}
#[tokio::test]
async fn rule_check_history_inbox_and_versions_form_a_complete_http_workflow() {
    let (shared, source, app) = setup(false).await;
    let id = create(&app).await;
    assert_eq!(source.calls.load(Ordering::SeqCst), 0);
    let (_, listed) = call(&app, "GET", "/subscriptions", None).await;
    assert_eq!(listed["rules"].as_array().unwrap().len(), 1);
    let run = uuid::Uuid::new_v4().to_string();
    assert_eq!(
        call(
            &app,
            "POST",
            &format!("/subscriptions/{id}/check"),
            Some(json!({"run_id":run}))
        )
        .await
        .0,
        StatusCode::ACCEPTED
    );
    assert_eq!(
        finished(&shared, &run).await.state,
        SubscriptionRunState::Complete
    );
    assert_eq!(shared.bib.article_count().await.unwrap(), 0);
    let (_, history) = call(
        &app,
        "GET",
        &format!("/subscriptions/{id}/runs?limit=1"),
        None,
    )
    .await;
    assert_eq!(history["runs"][0]["id"], run);
    let (_, inbox) = call(&app, "GET", "/subscriptions/inbox?limit=1", None).await;
    let item = inbox["items"][0]["id"].as_str().unwrap();
    assert_eq!(
        call(
            &app,
            "PUT",
            &format!("/subscriptions/inbox/{item}"),
            Some(json!({"read":true}))
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
    assert!(shared.subscriptions.inbox(None, 10).await.unwrap().0[0].read);
    assert_eq!(
        call(
            &app,
            "DELETE",
            &format!("/subscriptions/inbox/{item}"),
            None
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
    let mut replacement = spec();
    replacement["name"] = json!("Updated watch");
    assert_eq!(
        call(
            &app,
            "PUT",
            &format!("/subscriptions/{id}"),
            Some(json!({"version":1,"spec":replacement}))
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        call(
            &app,
            "POST",
            &format!("/subscriptions/{id}/enabled"),
            Some(json!({"version":1,"enabled":true}))
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        call(
            &app,
            "DELETE",
            &format!("/subscriptions/{id}?version=1"),
            None
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        call(
            &app,
            "DELETE",
            &format!("/subscriptions/{id}?version=2"),
            None
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        call(&app, "GET", &format!("/subscriptions/{id}"), None)
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    shared
        .subscription_service()
        .await
        .unwrap()
        .shutdown()
        .await;
}
#[tokio::test]
async fn run_uuid_retry_and_explicit_cancellation_never_duplicate_io() {
    let (shared, source, app) = setup(true).await;
    let rule = create(&app).await;
    let id = uuid::Uuid::new_v4().to_string();
    let path = format!("/subscriptions/{rule}/check");
    for _ in 0..2 {
        assert_eq!(
            call(&app, "POST", &path, Some(json!({"run_id":id})))
                .await
                .0,
            StatusCode::ACCEPTED
        );
    }
    tokio::time::timeout(Duration::from_secs(5), async {
        while source.calls.load(Ordering::SeqCst) == 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(source.calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        call(
            &app,
            "POST",
            &format!("/subscriptions/runs/{id}/cancel"),
            Some(json!({}))
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        finished(&shared, &id).await.state,
        SubscriptionRunState::Cancelled
    );
    let (status, retry) = call(&app, "POST", &path, Some(json!({"run_id":id}))).await;
    assert_eq!(status, StatusCode::ACCEPTED);
    assert_eq!(retry["run"]["state"], "cancelled");
    assert_eq!(
        call(&app, "DELETE", &format!("/subscriptions/runs/{id}"), None)
            .await
            .0,
        StatusCode::NO_CONTENT
    );
    shared
        .subscription_service()
        .await
        .unwrap()
        .shutdown()
        .await;
}
#[tokio::test]
async fn authentication_invalid_payloads_and_small_body_limit_are_enforced() {
    let (shared, source, app) = setup(false).await;
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/bib/subscriptions")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(
        call(
            &app,
            "POST",
            "/subscriptions",
            Some(json!({"name":"","request":{"query":"immune"}}))
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    let mut unknown = spec();
    unknown["unknown"] = json!(true);
    assert_eq!(
        call(&app, "POST", "/subscriptions", Some(unknown)).await.0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert_eq!(
        call(&app, "GET", "/subscriptions/inbox?limit=0", None)
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        call(&app, "GET", "/subscriptions/runs/missing", None)
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    let mut oversized = spec();
    oversized["name"] = json!("x".repeat(200_000));
    assert_eq!(
        call(&app, "POST", "/subscriptions", Some(oversized))
            .await
            .0,
        StatusCode::PAYLOAD_TOO_LARGE
    );
    assert_eq!(source.calls.load(Ordering::SeqCst), 0);
    shared
        .subscription_service()
        .await
        .unwrap()
        .shutdown()
        .await;
}
#[tokio::test]
async fn an_enabled_persisted_rule_resumes_on_host_startup_without_opening_subscription_view() {
    let dir = std::env::temp_dir().join(format!("bib-sub-api-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&dir).unwrap();
    let path = dir.join("bib.db");
    let shared = BibShared::open(&path).await.unwrap();
    let rule = shared
        .subscriptions
        .create(
            SubscriptionSpec {
                name: "Resume on startup".into(),
                enabled: true,
                request: DiscoveryRequest {
                    query: "immune".into(),
                    sources: vec!["openalex".into()],
                    ..Default::default()
                },
                ..Default::default()
            },
            bib_base::subscription_execution::now_ms(),
        )
        .await
        .unwrap();
    drop(shared);
    let mut reopened = BibShared::open(&path).await.unwrap();
    let source = Arc::new(Source {
        calls: AtomicUsize::new(0),
        hold: AtomicBool::new(false),
    });
    reopened.gateway = Arc::new(LiteratureGateway::new().with_source(source.clone()));
    let app = api_server::api_router_with_auth(
        reopened.clone(),
        None,
        Some("subscription-test-token".into()),
    );
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if reopened
                .subscriptions
                .get(&rule.id)
                .await
                .unwrap()
                .last_success_ms
                .is_some()
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(source.calls.load(Ordering::SeqCst), 1);
    reopened
        .subscription_service()
        .await
        .unwrap()
        .shutdown()
        .await;
    drop(app);
    drop(reopened);
    let _ = std::fs::remove_dir_all(dir);
}
