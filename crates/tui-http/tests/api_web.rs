//! Integration tests for the web-frontend facing bibliography endpoints:
//! paged article listings, collection trees, annotation payloads, settings,
//! chat transcripts, and single-article CSL-JSON.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use bib_base::BibShared;
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;

/// Percent-encode an article ID for use as a path segment. Article IDs look
/// like `doi:10.1000/foo`, and the slash must stay escaped or it ends the
/// route segment.
fn encode_id(id: &str) -> String {
    id.replace(':', "%3A").replace('/', "%2F")
}

fn request(method: &str, uri: &str, body: Option<String>) -> Request<Body> {
    let mut builder = Request::builder().method(method).uri(uri);
    if body.is_some() {
        builder = builder.header("content-type", "application/json");
    }
    builder
        .body(body.map(Body::from).unwrap_or_else(Body::empty))
        .unwrap()
}

/// Send a request and return `(status, parsed body)`.
async fn send(
    app: &axum::Router,
    method: &str,
    uri: &str,
    body: Option<String>,
) -> (StatusCode, Value) {
    let response = app
        .clone()
        .oneshot(request(method, uri, body))
        .await
        .unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, body)
}

async fn app() -> axum::Router {
    let shared = BibShared::open_in_memory().await.unwrap();
    tui_http::api_router(shared)
}

/// Create an article and return its ID.
async fn create_article(app: &axum::Router, title: &str, doi: &str, year: u16) -> String {
    let (status, body) = send(
        app,
        "POST",
        "/api/v1/bib/articles",
        Some(
            json!({
                "title": title,
                "doi": doi,
                "year": year,
                "journal": "Journal of Tests",
                "authors": ["Smith John"],
            })
            .to_string(),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "article body: {body}");
    body["article"]["id"].as_str().unwrap().to_owned()
}

#[tokio::test]
async fn article_listing_paginates_sorts_and_filters() {
    let app = app().await;

    for (title, doi) in [
        ("Alpha", "10.1000/alpha"),
        ("Bravo", "10.1000/bravo"),
        ("Charlie", "10.1000/charlie"),
        ("Delta", "10.1000/delta"),
        ("Echo", "10.1000/echo"),
    ] {
        create_article(&app, title, doi, 2024).await;
    }

    // Title sort is the only one with fully deterministic keys here (the
    // articles are created within the same clock tick), so it is the one we
    // assert end to end.
    let (status, body) = send(
        &app,
        "GET",
        "/api/v1/bib/articles?sort=title&order=asc&limit=2000",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["total"], 5);
    let titles = body["articles"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["title"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(titles, vec!["Alpha", "Bravo", "Charlie", "Delta", "Echo"]);
    // The pagination echo tells the client which slice it received.
    assert_eq!(body["offset"], 0);
    assert_eq!(body["limit"], 2000);

    let (status, body) = send(
        &app,
        "GET",
        "/api/v1/bib/articles?sort=title&order=desc&limit=2",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let titles = body["articles"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["title"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(titles, vec!["Echo", "Delta"]);

    // Offset slicing must agree with the unpaginated listing of the same
    // sort, whatever order the equal-ish created_at timestamps produce.
    let (status, full) = send(
        &app,
        "GET",
        "/api/v1/bib/articles?sort=created_at&order=desc&limit=2000",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let expected = full["articles"]
        .as_array()
        .unwrap()
        .iter()
        .skip(2)
        .take(2)
        .map(|a| a["id"].as_str().unwrap())
        .collect::<Vec<_>>();
    let (status, page) = send(
        &app,
        "GET",
        "/api/v1/bib/articles?sort=created_at&order=desc&offset=2&limit=2",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let got = page["articles"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["id"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(got, expected);
    assert_eq!(page["offset"], 2);
    assert_eq!(page["limit"], 2);
    // `total` stays the whole filtered count, not the page size.
    assert_eq!(page["total"], 5);

    // Bad sort/order values are rejected instead of silently reordered.
    let (status, body) = send(&app, "GET", "/api/v1/bib/articles?sort=colour", None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body["error"].as_str().unwrap().contains("sort"));
    let (status, _) = send(&app, "GET", "/api/v1/bib/articles?order=sideways", None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Collection filter.
    let (status, body) = send(
        &app,
        "POST",
        "/api/v1/bib/collections",
        Some(json!({"name": "Reading"}).to_string()),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let collection_id = body["collection"]["id"].as_str().unwrap().to_owned();
    for doi in ["10.1000/bravo", "10.1000/charlie"] {
        let article_id = format!("doi:{doi}");
        let (status, body) = send(
            &app,
            "POST",
            &format!("/api/v1/bib/collections/{collection_id}/articles"),
            Some(json!({"article_id": article_id}).to_string()),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
    }

    let (status, body) = send(
        &app,
        "GET",
        &format!("/api/v1/bib/articles?collection_id={collection_id}&sort=title&order=asc&limit=2000"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let articles = body["articles"].as_array().unwrap();
    let titles = articles
        .iter()
        .map(|a| a["title"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(titles, vec!["Bravo", "Charlie"]);
    assert_eq!(body["total"], 2);
    // `hits` stays aligned with `articles` for pre-pagination clients.
    assert_eq!(
        body["hits"].as_array().unwrap().len(),
        articles.len(),
        "hits and articles must stay index-aligned"
    );

    // Unfiled excludes anything in a collection.
    let (status, body) = send(
        &app,
        "GET",
        "/api/v1/bib/articles?unfiled=true&limit=2000",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let mut titles = body["articles"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["title"].as_str().unwrap())
        .collect::<Vec<_>>();
    titles.sort();
    assert_eq!(titles, vec!["Alpha", "Delta", "Echo"]);
    assert_eq!(body["total"], 3);
}

#[tokio::test]
async fn collections_nest_move_and_reject_cycles() {
    let app = app().await;

    let (status, body) = send(
        &app,
        "POST",
        "/api/v1/bib/collections",
        Some(json!({"name": "Parent"}).to_string()),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let parent = body["collection"]["id"].as_str().unwrap().to_owned();

    let (status, body) = send(
        &app,
        "POST",
        "/api/v1/bib/collections",
        Some(
            json!({"name": "Child", "parent_id": parent, "sort_order": 3}).to_string(),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(body["collection"]["parent_id"], parent.as_str());
    assert_eq!(body["collection"]["sort_order"], 3);
    let child = body["collection"]["id"].as_str().unwrap().to_owned();

    // Rename in place, keeping the parent.
    let (status, body) = send(
        &app,
        "PUT",
        &format!("/api/v1/bib/collections/{child}"),
        Some(json!({"name": "Child renamed", "description": "under parent"}).to_string()),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["collection"]["name"], "Child renamed");
    assert_eq!(body["collection"]["parent_id"], parent.as_str());
    assert_eq!(body["collection"]["description"], "under parent");

    // Move under a second root.
    let (status, body) = send(
        &app,
        "POST",
        "/api/v1/bib/collections",
        Some(json!({"name": "Other root"}).to_string()),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let other = body["collection"]["id"].as_str().unwrap().to_owned();

    let (status, body) = send(
        &app,
        "PUT",
        &format!("/api/v1/bib/collections/{child}"),
        Some(json!({"parent_id": other}).to_string()),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["collection"]["parent_id"], other.as_str());

    // Clearing the parent promotes the collection back to a root.
    let (status, body) = send(
        &app,
        "PUT",
        &format!("/api/v1/bib/collections/{child}"),
        Some(json!({"parent_id": null}).to_string()),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body["collection"]["parent_id"].is_null());

    // A collection cannot be its own parent.
    let (status, body) = send(
        &app,
        "PUT",
        &format!("/api/v1/bib/collections/{parent}"),
        Some(json!({"parent_id": parent}).to_string()),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"], "collection cycle detected");

    // Now walk A under B, then try to walk B under A.
    let (status, _) = send(
        &app,
        "PUT",
        &format!("/api/v1/bib/collections/{parent}"),
        Some(json!({"parent_id": child}).to_string()),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status, body) = send(
        &app,
        "PUT",
        &format!("/api/v1/bib/collections/{child}"),
        Some(json!({"parent_id": parent}).to_string()),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["error"], "collection cycle detected");

    // A missing parent is a client error, not an empty tree node.
    let (status, _) = send(
        &app,
        "PUT",
        &format!("/api/v1/bib/collections/{child}"),
        Some(json!({"parent_id": "col-missing"}).to_string()),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Empty names stay rejected on update.
    let (status, _) = send(
        &app,
        "PUT",
        &format!("/api/v1/bib/collections/{child}"),
        Some(json!({"name": "   "}).to_string()),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // The flat listing carries the new fields.
    let (status, body) = send(&app, "GET", "/api/v1/bib/collections", None).await;
    assert_eq!(status, StatusCode::OK);
    let listed = body["collections"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["id"] == parent.as_str())
        .unwrap();
    assert!(listed.get("parent_id").is_some());
    assert!(listed.get("sort_order").is_some());
}

#[tokio::test]
async fn annotations_store_payloads_and_update_in_place() {
    let app = app().await;
    let article_id = create_article(&app, "Highlight target", "10.1000/highlight-target", 2025).await;
    let encoded = encode_id(&article_id);

    let (status, body) = send(
        &app,
        "POST",
        &format!("/api/v1/bib/articles/{encoded}/annotations"),
        Some(
            json!({
                "content": "key sentence",
                "kind": "highlight",
                "page": 3,
                "data": {"rects": [{"x": 1.0, "y": 2.0}], "color": "#ff0"},
            })
            .to_string(),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let annotation_id = body["annotation"]["id"].as_str().unwrap().to_owned();
    assert_eq!(body["annotation"]["page"], 3);
    assert_eq!(body["annotation"]["data"]["color"], "#ff0");
    assert_eq!(body["annotation"]["data"]["rects"].as_array().unwrap().len(), 1);

    let (status, body) = send(
        &app,
        "GET",
        &format!("/api/v1/bib/articles/{encoded}/annotations?page=3"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["annotations"].as_array().unwrap().len(), 1);

    let (status, body) = send(
        &app,
        "GET",
        &format!("/api/v1/bib/articles/{encoded}/annotations?page=4"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["annotations"].as_array().unwrap().len(), 0);

    // Partial update: only data and page move, content is preserved.
    let (status, body) = send(
        &app,
        "PUT",
        &format!("/api/v1/bib/annotations/{annotation_id}"),
        Some(
            json!({
                "page": 5,
                "data": {"rects": [{"x": 3.0}, {"x": 4.0}], "color": "#0f0"},
            })
            .to_string(),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["annotation"]["page"], 5);
    assert_eq!(body["annotation"]["content"], "key sentence");
    assert_eq!(body["annotation"]["data"]["color"], "#0f0");
    assert_eq!(body["annotation"]["data"]["rects"].as_array().unwrap().len(), 2);

    let (status, body) = send(
        &app,
        "GET",
        &format!("/api/v1/bib/articles/{encoded}/annotations?page=5"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let page_five = body["annotations"].as_array().unwrap();
    assert_eq!(page_five.len(), 1);
    assert_eq!(page_five[0]["data"]["color"], "#0f0");

    // `null` clears an optional field without touching the rest.
    let (status, body) = send(
        &app,
        "PUT",
        &format!("/api/v1/bib/annotations/{annotation_id}"),
        Some(json!({"data": null}).to_string()),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body["annotation"]["data"].is_null());
    assert_eq!(body["annotation"]["page"], 5);

    let (status, body) = send(
        &app,
        "PUT",
        &format!("/api/v1/bib/annotations/{annotation_id}"),
        Some(json!({"content": "   "}).to_string()),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");

    let (status, body) = send(
        &app,
        "PUT",
        "/api/v1/bib/annotations/ann-missing",
        Some(json!({"content": "nope"}).to_string()),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(body["error"].as_str().unwrap().contains("ann-missing"));
}

#[tokio::test]
async fn settings_round_trip_and_merge() {
    let app = app().await;

    // An empty store reads back as an empty object, not an error.
    let (status, body) = send(&app, "GET", "/api/v1/bib/settings", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["settings"], json!({}));

    let (status, _) = send(
        &app,
        "PUT",
        "/api/v1/bib/settings",
        Some(json!({"settings": {"a": 1, "b": "x"}}).to_string()),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status, body) = send(&app, "GET", "/api/v1/bib/settings", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["settings"], json!({"a": 1, "b": "x"}));

    // A partial write must not clobber keys another tab owns.
    let (status, _) = send(
        &app,
        "PUT",
        "/api/v1/bib/settings",
        Some(json!({"settings": {"c": true}}).to_string()),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status, body) = send(&app, "GET", "/api/v1/bib/settings", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["settings"], json!({"a": 1, "b": "x", "c": true}));

    // An empty object is a legal no-op write.
    let (status, _) = send(
        &app,
        "PUT",
        "/api/v1/bib/settings",
        Some(json!({"settings": {}}).to_string()),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, body) = send(&app, "GET", "/api/v1/bib/settings", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["settings"], json!({"a": 1, "b": "x", "c": true}));

    // `chat:` is reserved for transcripts, so settings cannot collide with
    // (or be clobbered by) a chat blob stored under the same key.
    let (status, _) = send(
        &app,
        "PUT",
        "/api/v1/bib/settings",
        Some(json!({"settings": {"chat:conv-1": {"stolen": true}}}).to_string()),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, body) = send(&app, "GET", "/api/v1/bib/chat?scope=conv-1", None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body["payload"].is_null(), "{body}");
}

#[tokio::test]
async fn chat_transcripts_persist_per_scope() {
    let app = app().await;

    // Unknown scope reads back as a null payload.
    let (status, body) = send(&app, "GET", "/api/v1/bib/chat?scope=fresh", None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body["payload"].is_null());

    let transcript = json!({"messages": [{"role": "user", "content": "hello"}]});
    let (status, body) = send(
        &app,
        "POST",
        "/api/v1/bib/chat",
        Some(json!({"scope": "conv-1", "payload": transcript}).to_string()),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["ok"], true);

    let (status, body) = send(&app, "GET", "/api/v1/bib/chat?scope=conv-1", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["payload"], transcript);

    // Overwrite wins wholesale.
    let replacement = json!({"messages": [{"role": "assistant", "content": "hi"}]});
    let (status, _) = send(
        &app,
        "POST",
        "/api/v1/bib/chat",
        Some(json!({"scope": "conv-1", "payload": replacement}).to_string()),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status, body) = send(&app, "GET", "/api/v1/bib/chat?scope=conv-1", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["payload"], replacement);

    // Validation: missing scope, empty scope, oversized payload.
    let (status, _) = send(&app, "GET", "/api/v1/bib/chat", None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let (status, _) = send(
        &app,
        "POST",
        "/api/v1/bib/chat",
        Some(json!({"scope": "", "payload": {}}).to_string()),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let oversized = "x".repeat(5 * 1024 * 1024 + 1);
    let (status, _) = send(
        &app,
        "POST",
        "/api/v1/bib/chat",
        Some(json!({"scope": "conv-1", "payload": {"blob": oversized}}).to_string()),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // The rejected write must not have replaced the stored transcript.
    let (status, body) = send(&app, "GET", "/api/v1/bib/chat?scope=conv-1", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["payload"], replacement);
}

#[tokio::test]
async fn article_csl_json_and_missing_article() {
    let app = app().await;
    let article_id = create_article(&app, "Citation target", "10.1000/csl-target", 2026).await;
    let encoded = encode_id(&article_id);

    let (status, body) = send(&app, "GET", &format!("/api/v1/bib/articles/{encoded}/csl-json"), None).await;
    assert_eq!(status, StatusCode::OK);
    let csl = &body["csl_json"];
    assert_eq!(csl["DOI"], "10.1000/csl-target");
    assert_eq!(csl["title"], "Citation target");
    assert_eq!(csl["type"], "article-journal");
    assert_eq!(csl["author"].as_array().unwrap().len(), 1);

    let (status, body) = send(
        &app,
        "GET",
        &format!("/api/v1/bib/articles/{}/csl-json", encode_id("doi:10.1000/absent")),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(body["error"].as_str().unwrap().contains("absent"));
}
