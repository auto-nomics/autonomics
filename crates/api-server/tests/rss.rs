//! RSS subscription routes: feed CRUD, inbox paging, import into library.
use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode},
};
use bib_base::BibShared;
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
fn spec(url: &str) -> Value {
    json!({"name": "Fixture feed", "url": url, "enabled": true, "interval_seconds": 3600, "max_items_per_feed": 100})
}

#[tokio::test]
async fn feed_lifecycle_inbox_and_import_flow() {
    let shared = Arc::new(BibShared::open_in_memory().await.unwrap());
    let app = api_server::bib::router((*shared).clone());

    // Create + duplicate rejection.
    let (status, result) = call(
        &app,
        "POST",
        "/rss/feeds",
        Some(spec("http://127.0.0.1:9/feed")),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{result}");
    let feed = result["feed"].clone();
    assert_eq!(feed["version"], 0);
    let (status, _) = call(
        &app,
        "POST",
        "/rss/feeds",
        Some(spec("http://127.0.0.1:9/feed")),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    // Invalid spec.
    let (status, _) = call(&app, "POST", "/rss/feeds", Some(json!({"name":"x","url":"ftp://nope","enabled":true,"interval_seconds":3600,"max_items_per_feed":100}))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    // Status requires the scheduler to be running.
    let (status, result) = call(&app, "GET", "/rss/status", None).await;
    assert_eq!(status, StatusCode::OK, "{result}");
    assert_eq!(result["scheduler"]["running"], true);
    // Manual check against an unreachable origin reports a per-feed error.
    let id = feed["id"].as_str().unwrap().to_owned();
    let (status, result) = call(&app, "POST", &format!("/rss/feeds/{id}"), None).await;
    assert_eq!(status, StatusCode::OK, "{result}");
    assert_eq!(result["outcome"]["status"], "error");
    // Optimistic-concurrency replace/pause/delete.
    let version = feed["version"].as_u64().unwrap();
    let (status, _) = call(
        &app,
        "PUT",
        &format!("/rss/feeds/{id}"),
        Some(json!({"version": 99, "spec": spec("http://127.0.0.1:9/feed")})),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    let (status, updated) = call(
        &app,
        "PUT",
        &format!("/rss/feeds/{id}"),
        Some(json!({"version": version, "spec": spec("http://127.0.0.1:9/feed2")})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(updated["feed"]["spec"]["url"], "http://127.0.0.1:9/feed2");
    let version = updated["feed"]["version"].as_u64().unwrap();
    let (status, paused) = call(
        &app,
        "POST",
        &format!("/rss/feeds/{id}/enabled"),
        Some(json!({"version": version, "enabled": false})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(paused["feed"]["spec"]["enabled"], false);
    let (status, _) = call(
        &app,
        "DELETE",
        &format!(
            "/rss/feeds/{id}?version={}",
            paused["feed"]["version"].as_u64().unwrap()
        ),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = call(&app, "GET", &format!("/rss/feeds/{id}"), None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn inbox_items_support_read_delete_and_library_import() {
    let shared = Arc::new(BibShared::open_in_memory().await.unwrap());
    let app = api_server::bib::router((*shared).clone());
    // Seed one feed + one item directly through the store.
    let feed = shared
        .rss
        .create_feed(serde_json::from_value(spec("http://127.0.0.1:9/feed")).unwrap())
        .await
        .unwrap();
    shared
        .rss
        .commit_check(
            &feed,
            &bib_base::rss::CheckOutcome {
                feed_id: feed.id.clone(),
                status: "fetched",
                new_items: 1,
                total_items: 1,
                error: None,
            },
            &[bib_base::rss::ParsedItem {
                guid: "g1".into(),
                title: "Inbox paper".into(),
                link: Some("https://example.org/abs/10.1000/rss-one".into()),
                summary: Some("A summary".into()),
                authors: vec!["Lovelace, Ada".into()],
                published_ms: Some(1_767_000_000_000),
            }],
        )
        .await
        .unwrap();
    let (status, result) = call(&app, "GET", "/rss/items?limit=10", None).await;
    assert_eq!(status, StatusCode::OK, "{result}");
    let item = result["items"][0].clone();
    assert_eq!(item["title"], "Inbox paper");
    assert_eq!(item["read"], false);
    let item_id = item["id"].as_str().unwrap().to_owned();

    // Mark read, then unread.
    let (status, _) = call(
        &app,
        "PUT",
        &format!("/rss/items/{item_id}"),
        Some(json!({"read": true})),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (_, result) = call(&app, "GET", &format!("/rss/items/{item_id}"), None).await;
    assert_eq!(result["item"]["read"], true);

    // Import into the library; a DOI is extracted from the link.
    let (status, result) = call(
        &app,
        "POST",
        &format!("/rss/items/{item_id}"),
        Some(json!({})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{result}");
    let article_id = result["article_id"].as_str().unwrap().to_owned();
    let article = shared.bib.get_article(&article_id).await.unwrap().unwrap();
    assert!(
        article
            .identifiers
            .iter()
            .any(|i| i.value == "10.1000/rss-one")
    );
    // Import is idempotent.
    let (_, again) = call(
        &app,
        "POST",
        &format!("/rss/items/{item_id}"),
        Some(json!({})),
    )
    .await;
    assert_eq!(again["article_id"], article_id.as_str());
    assert_eq!(again["already_imported"], true);

    // Delete the item.
    let (status, _) = call(&app, "DELETE", &format!("/rss/items/{item_id}"), None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = call(&app, "GET", &format!("/rss/items/{item_id}"), None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}
