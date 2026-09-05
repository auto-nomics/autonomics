//! Integration tests for the web-frontend facing bibliography endpoints:
//! paged article listings, collection trees, annotation payloads, settings,
//! chat transcripts, and single-article CSL-JSON.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use bib_base::BibShared;
use http_body_util::BodyExt;
use serde_json::{Value, json};
use std::sync::Arc;
use tower::ServiceExt;
use vfs::{
    BackendConfig, BackendDefinition, MountDefinition, MountedObjectStore, OpendalFileStorage,
    VfsManifest,
};

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

/// Shared bundle whose uploads can actually persist: the VFS storage is
/// mounted on a temp directory, like the TUI's literature mount.
async fn shared_with_storage() -> BibShared {
    let directory = tempfile::tempdir().unwrap();
    let manifest = VfsManifest {
        backend: vec![BackendDefinition {
            id: "literature".to_owned(),
            config: BackendConfig::local(
                directory
                    .path()
                    .join("literature")
                    .to_string_lossy()
                    .to_string(),
            ),
        }],
        mount: vec![MountDefinition {
            path: "/literature".to_owned(),
            backend: "literature".to_owned(),
            source: "/".to_owned(),
            read_only: false,
        }],
    };
    let vfs = Arc::new(MountedObjectStore::from_manifest(&manifest).unwrap());
    let file_storage = Arc::new(OpendalFileStorage::with_mounts(directory.path(), vfs));
    let shared = BibShared::open_in_memory()
        .await
        .unwrap()
        .with_file_storage(file_storage);
    // Keep the temp directory alive for the router's lifetime by leaking it —
    // the test process is short-lived.
    std::mem::forget(directory);
    shared
}

/// App whose uploads can actually persist: the VFS storage is mounted on a
/// temp directory, like the TUI's literature mount.
async fn app_with_storage() -> axum::Router {
    tui_http::api_router(shared_with_storage().await)
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

// --- Upload & batch import (Phase 3) ---------------------------------------

/// Build a `multipart/form-data` request; the file part is omitted when
/// `filename` is empty, covering the missing-field rejection path.
fn multipart_request(
    uri: &str,
    filename: &str,
    content: &str,
    extra_fields: &[(&str, &str)],
) -> Request<Body> {
    let boundary = "autonomics-test-boundary";
    let mut body = Vec::new();
    for (name, value) in extra_fields {
        body.extend_from_slice(
            format!(
                "--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n"
            )
            .as_bytes(),
        );
    }
    if !filename.is_empty() {
        body.extend_from_slice(
            format!(
                "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; \
                 filename=\"{filename}\"\r\nContent-Type: text/plain\r\n\r\n{content}\r\n\
                 --{boundary}--\r\n"
            )
            .as_bytes(),
        );
    } else {
        body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    }
    Request::builder()
        .method("POST")
        .uri(uri)
        .header(
            "content-type",
            format!("multipart/form-data; boundary={boundary}"),
        )
        .body(Body::from(body))
        .unwrap()
}

async fn send_multipart(
    app: &axum::Router,
    filename: &str,
    content: &str,
    extra_fields: &[(&str, &str)],
) -> (StatusCode, Value) {
    send_multipart_uri(app, "/api/v1/bib/articles/upload", filename, content, extra_fields).await
}

/// [`send_multipart`] for a target other than the upload endpoint (e.g.
/// attaching a file to an existing article).
async fn send_multipart_uri(
    app: &axum::Router,
    uri: &str,
    filename: &str,
    content: &str,
    extra_fields: &[(&str, &str)],
) -> (StatusCode, Value) {
    let response = app
        .clone()
        .oneshot(multipart_request(uri, filename, content, extra_fields))
        .await
        .unwrap();
    let status = response.status();
    let bytes = response
        .into_body()
        .collect()
        .await
        .unwrap()
        .to_bytes();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

#[tokio::test]
async fn upload_without_identifiers_creates_a_manual_article() {
    let app = app_with_storage().await;

    let (status, body) =
        send_multipart(&app, "field-notes.txt", "Plain prose with no identifiers.", &[]).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["created"], true);
    assert!(body["identifier"].is_null());

    let article = &body["article"];
    assert!(article["id"].as_str().unwrap().starts_with("local:"));
    assert_eq!(article["title"], "field-notes");
    // The document itself is retrievable as extracted text.
    let encoded = encode_id(article["id"].as_str().unwrap());
    let (status, body) = send(
        &app,
        "GET",
        &format!("/api/v1/bib/articles/{encoded}/fulltext"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["fulltext"]["text_content"], "Plain prose with no identifiers.");
}

#[tokio::test]
async fn upload_with_a_doi_keys_the_article_by_the_identifier() {
    let app = app_with_storage().await;

    // The gateway cannot resolve this made-up prefix, so the stub path runs.
    let (status, body) = send_multipart(
        &app,
        "paper.pdf.txt",
        "Preprint title.\nDOI: 10.9999/offline-doi-test.",
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["created"], true);
    assert_eq!(body["identifier"]["kind"], "doi");
    assert_eq!(
        body["article"]["id"].as_str().unwrap(),
        "doi:10.9999/offline-doi-test"
    );

    // Re-uploading attaches to the existing article instead of duplicating.
    let (status, body) = send_multipart(
        &app,
        "paper-again.pdf.txt",
        "DOI: 10.9999/offline-doi-test repeated.",
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["created"], false);
    assert_eq!(
        body["article"]["id"].as_str().unwrap(),
        "doi:10.9999/offline-doi-test"
    );
}

#[tokio::test]
async fn upload_annotates_a_missing_category_and_requires_a_file() {
    let app = app().await;

    let (status, body) = send_multipart(&app, "x.txt", "text", &[("category_id", "nope")]).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(body["error"].as_str().unwrap().contains("nope"));

    let response = app
        .clone()
        .oneshot(multipart_request(
            "/api/v1/bib/articles/upload",
            "", // no file part at all
            "",
            &[("category_id", "")],
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn upload_files_into_a_category() {
    let app = app_with_storage().await;
    let (status, body) = send(
        &app,
        "POST",
        "/api/v1/bib/collections",
        Some(json!({"name": "Uploads"}).to_string()),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let collection = body["collection"]["id"].as_str().unwrap().to_owned();

    let (status, body) = send_multipart(
        &app,
        "categorized.txt",
        "No identifiers here either.",
        &[("category_id", collection.as_str())],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let article_id = body["article"]["id"].as_str().unwrap().to_owned();

    let (status, body) = send(
        &app,
        "GET",
        &format!("/api/v1/bib/collections/{collection}/articles"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let articles = body["articles"].as_array().unwrap();
    assert_eq!(articles.len(), 1);
    assert_eq!(articles[0]["id"].as_str().unwrap(), article_id);
}

#[tokio::test]
async fn batch_import_merges_duplicates_and_reports_failures() {
    let app = app().await;
    // Pre-store one of the entries' DOIs so the batch hits a real duplicate.
    create_article(&app, "Stored original", "10.1000/already-here", 2020).await;

    let bibtex = "\
@article{dup1, title = {The same paper}, doi = {10.1000/already-here}}
@article{fresh1, title = {A new paper}, doi = {10.1000/batch-fresh}, year = {2024}}
@article{twice1, title = {Also new}, doi = {10.1000/batch-fresh}}
@article{empty1, author = {Nobody}}
";
    let (status, body) = send(
        &app,
        "POST",
        "/api/v1/bib/articles/import/batch",
        Some(json!({"format": "bibtex", "content": bibtex}).to_string()),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    // fresh1 imported, twice1 merged into it (batch-internal duplicate),
    // dup1 merged into the stored article, empty1 failed.
    assert_eq!(body["imported"], 1);
    assert_eq!(body["duplicates"], 2);
    assert_eq!(body["failed"].as_array().unwrap().len(), 1);
    assert_eq!(body["failed"][0]["key"], "empty1");
    assert_eq!(
        body["articles"][0]["id"].as_str().unwrap(),
        "doi:10.1000/batch-fresh"
    );

    let details = body["duplicate_details"].as_array().unwrap();
    assert!(details
        .iter()
        .any(|detail| detail["existing_id"].as_str().unwrap() == "doi:10.1000/already-here"));

    // A second identical run now reports everything with a title as a
    // duplicate — including fresh1, imported by the first run.
    let (status, body) = send(
        &app,
        "POST",
        "/api/v1/bib/articles/import/batch",
        Some(json!({"format": "bibtex", "content": bibtex}).to_string()),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["imported"], 0);
    assert_eq!(body["duplicates"], 3);
}

#[tokio::test]
async fn batch_import_accepts_ris_and_csl_json_and_auto() {
    let app = app().await;

    let ris = "TY  - JOUR\nTI  - RIS item\nDO  - 10.2000/ris-import\nER  - \n";
    let (status, body) = send(
        &app,
        "POST",
        "/api/v1/bib/articles/import/batch",
        Some(json!({"format": "ris", "content": ris}).to_string()),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["imported"], 1);
    assert_eq!(
        body["articles"][0]["id"].as_str().unwrap(),
        "doi:10.2000/ris-import"
    );

    let csl = r#"[{"id": "doi:10.3000/csl-import", "title": "CSL item", "DOI": "10.3000/csl-import", "issued": {"date-parts": [[2025]]}}]"#;
    let (status, body) = send(
        &app,
        "POST",
        "/api/v1/bib/articles/import/batch",
        Some(json!({"format": "auto", "content": csl}).to_string()),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["imported"], 1);
    assert_eq!(body["articles"][0]["year"], 2025);

    // Unknown format names and undetectable content are 400s.
    let (status, _) = send(
        &app,
        "POST",
        "/api/v1/bib/articles/import/batch",
        Some(json!({"format": "endnote-xml", "content": "<xml/>"}).to_string()),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let (status, _) = send(
        &app,
        "POST",
        "/api/v1/bib/articles/import/batch",
        Some(json!({"format": "auto", "content": "not a bibliography"}).to_string()),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn batch_import_files_into_a_category() {
    let app = app().await;
    let (status, body) = send(
        &app,
        "POST",
        "/api/v1/bib/collections",
        Some(json!({"name": "Imported"}).to_string()),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let collection = body["collection"]["id"].as_str().unwrap().to_owned();

    let (status, body) = send(
        &app,
        "POST",
        "/api/v1/bib/articles/import/batch",
        Some(
            json!({
                "format": "auto",
                "content": "@article{k1, title = {Filed one}}\n@article{k2, title = {Filed two}}",
                "category_id": collection,
            })
            .to_string(),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["imported"], 2);

    let (status, body) = send(
        &app,
        "GET",
        &format!("/api/v1/bib/collections/{collection}/articles"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["articles"].as_array().unwrap().len(), 2);
}

// --- MinerU async parse pipeline --------------------------------------------

/// Fake MinerU result zip: `full.md` + `content_list.json` under the usual
/// `{name}/` directory prefix.
fn mineru_result_zip() -> Vec<u8> {
    use std::io::Write;
    let buffer = std::io::Cursor::new(Vec::new());
    let mut zip = zip::ZipWriter::new(buffer);
    zip.start_file::<_, ()>(
        "paper/full.md".to_owned(),
        zip::write::SimpleFileOptions::default(),
    )
    .unwrap();
    zip.write_all(b"# Mock MinerU Title\n\nParsed markdown body.")
        .unwrap();
    zip.start_file::<_, ()>(
        "paper/content_list.json".to_owned(),
        zip::write::SimpleFileOptions::default(),
    )
    .unwrap();
    zip.write_all(
        br#"[{"type":"title","text":"Mock MinerU Title","page_idx":0,"text_level":1}]"#,
    )
    .unwrap();
    zip.finish().unwrap().into_inner()
}

/// How the fake extract-results endpoint answers.
#[derive(Clone, Copy, PartialEq)]
enum MockMineruMode {
    /// running(1/2) → running(2/2) → done.
    ProgressThenDone,
    /// Stays running forever — for the reparse-409 concurrency guard.
    NeverDone,
}

struct MockMineru {
    polls: std::sync::atomic::AtomicU32,
    uploaded: std::sync::Mutex<Option<Vec<u8>>>,
}

/// Spawn an in-process fake of the MinerU v4 API on a random port and return
/// `(base_url, state)`. Mirrors the four wire calls the real service makes:
/// URL claim, bare PUT upload, batch polling, zip download.
async fn spawn_mock_mineru(mode: MockMineruMode) -> (String, Arc<MockMineru>) {
    use axum::routing::{get, put};
    use std::sync::atomic::Ordering;

    let state = Arc::new(MockMineru {
        polls: std::sync::atomic::AtomicU32::new(0),
        uploaded: std::sync::Mutex::new(None),
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let zip_bytes = Arc::new(mineru_result_zip());

    let upload_state = Arc::clone(&state);
    let poll_state = Arc::clone(&state);
    let app: axum::Router = axum::Router::new()
        .route(
            "/api/v4/file-urls/batch",
            axum::routing::post(move || {
                let addr = addr.clone();
                async move {
                    axum::Json(json!({
                        "code": 0,
                        "msg": "ok",
                        "data": {
                            "batch_id": "batch-1",
                            "file_urls": [format!("http://{addr}/upload")],
                        }
                    }))
                }
            }),
        )
        .route(
            "/upload",
            put(move |body: axum::body::Bytes| {
                let state = Arc::clone(&upload_state);
                async move {
                    *state.uploaded.lock().unwrap() = Some(body.to_vec());
                    StatusCode::OK
                }
            }),
        )
        .route(
            "/api/v4/extract-results/batch/batch-1",
            get(move || {
                let state = Arc::clone(&poll_state);
                let zip_url = format!("http://{addr}/result.zip");
                async move {
                    let poll = state.polls.fetch_add(1, Ordering::SeqCst) + 1;
                    let (name, progress, url) = match mode {
                        MockMineruMode::ProgressThenDone if poll >= 3 => {
                            ("done", None, Some(zip_url))
                        }
                        MockMineruMode::ProgressThenDone => {
                            ("running", Some((poll.min(2) as u64, 2)), None)
                        }
                        MockMineruMode::NeverDone => ("running", Some((1, 2)), None),
                    };
                    let mut item = json!({ "state": name, "err_msg": "" });
                    if let Some((extracted, total)) = progress {
                        item["extract_progress"] = json!({
                            "extracted_pages": extracted,
                            "total_pages": total,
                        });
                    }
                    if let Some(url) = url {
                        item["full_zip_url"] = json!(url);
                    }
                    axum::Json(json!({
                        "code": 0,
                        "msg": "ok",
                        "data": {
                            "batch_id": "batch-1",
                            "extract_result": [item],
                        }
                    }))
                }
            }),
        )
        .route(
            "/result.zip",
            get(move || async move { (*zip_bytes).clone() }),
        );

    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (format!("http://{addr}"), state)
}

/// App + shared bundle with a temp-dir VFS and the MinerU client pointed at
/// an in-process mock (30 ms poll cadence so the pipeline settles fast).
async fn app_with_mock_mineru(mode: MockMineruMode) -> (axum::Router, Arc<MockMineru>, BibShared) {
    let shared = shared_with_storage().await;
    let (base, mock) = spawn_mock_mineru(mode).await;
    shared.parse_hub.mineru.set_key(Some("test-token".to_owned()));
    shared.parse_hub.mineru.set_base_url(base);
    shared
        .parse_hub
        .mineru
        .set_poll_interval(std::time::Duration::from_millis(30));
    (tui_http::api_router(shared.clone()), mock, shared)
}

#[tokio::test]
async fn pdf_upload_parses_asynchronously_through_mineru() {
    let (app, mock, _) = app_with_mock_mineru(MockMineruMode::ProgressThenDone).await;

    // Garbage PDF bytes: the local quick-extract finds no identifier, so a
    // manual article is created — the parse itself is entirely the mock's.
    let (status, body) = send_multipart(&app, "paper.pdf", "%PDF-1.4 mock pdf bytes", &[]).await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    assert_eq!(body["fulltext"]["parse_status"], "pending");
    assert_eq!(body["text_chars"], 0);
    let id = body["article"]["id"].as_str().unwrap().to_owned();

    // The background task settles the row to done.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    let settled = loop {
        let (status, body) = send(&app, "GET", "/api/v1/bib/fulltext-statuses", None).await;
        assert_eq!(status, StatusCode::OK);
        let entry = body["statuses"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["article_id"].as_str() == Some(id.as_str()))
            .expect("status row exists")
            .clone();
        if entry["parse_status"] == "done" {
            break entry;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "parse did not settle in time: {entry}"
        );
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    };
    assert_eq!(settled["parse_engine"], "mineru");

    // The PDF bytes actually reached the fake MinerU upload.
    assert!(mock
        .uploaded
        .lock()
        .unwrap()
        .as_deref()
        .is_some_and(|bytes| bytes.starts_with(b"%PDF")));

    // The mock's markdown is readable through the normal fulltext endpoint.
    let (status, body) = send(
        &app,
        "GET",
        &format!("/api/v1/bib/articles/{}/fulltext", encode_id(&id)),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["fulltext"]["parse_status"], "done");
    assert_eq!(body["fulltext"]["parse_engine"], "mineru");
    assert!(body["fulltext"]["text_content"]
        .as_str()
        .unwrap()
        .contains("Mock MinerU Title"));
}

#[tokio::test]
async fn pdf_upload_without_a_mineru_token_is_rejected_up_front() {
    let shared = shared_with_storage().await;
    // Explicitly clear: MINERU_API_TOKEN may be set on a dev machine.
    shared.parse_hub.mineru.set_key(None);
    let app = tui_http::api_router(shared);

    let (status, body) = send_multipart(&app, "paper.pdf", "%PDF-1.4 bytes", &[]).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body["error"].as_str().unwrap().contains("MinerU API token"));

    // Nothing was stored — no article, no pending row.
    let (status, body) = send(&app, "GET", "/api/v1/bib/articles", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["articles"].as_array().unwrap().len(), 0);
    let (status, body) = send(&app, "GET", "/api/v1/bib/fulltext-statuses", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["statuses"].as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn parse_stream_emits_progress_and_done_events() {
    let (app, _, _) = app_with_mock_mineru(MockMineruMode::ProgressThenDone).await;

    let (status, body) = send_multipart(&app, "paper.pdf", "%PDF-1.4 mock pdf bytes", &[]).await;
    assert_eq!(status, StatusCode::OK);
    let stream_uri = format!(
        "/api/v1/bib/articles/{}/parse/stream",
        encode_id(body["article"]["id"].as_str().unwrap()),
    );

    // Open the stream while the parse is still polling: progress events flow,
    // then the terminal done ends the body.
    let response = app
        .clone()
        .oneshot(request("GET", &stream_uri, None))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(response
        .headers()
        .get("content-type")
        .unwrap()
        .to_str()
        .unwrap()
        .starts_with("text/event-stream"));
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let stream = String::from_utf8_lossy(&bytes);
    assert!(stream.contains("event: progress"), "stream: {stream}");
    assert!(stream.contains("event: done"), "stream: {stream}");
    assert!(stream.contains("mineru"), "stream: {stream}");

    // A second connection after the fact learns the outcome from the DB
    // snapshot instead of hanging on pings.
    let response = app
        .clone()
        .oneshot(request("GET", &stream_uri, None))
        .await
        .unwrap();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let replay = String::from_utf8_lossy(&bytes);
    assert!(replay.contains("event: done"), "replay: {replay}");
    assert!(!replay.contains("event: progress"), "replay: {replay}");
}

#[tokio::test]
async fn opening_the_parse_stream_resumes_an_interrupted_parse() {
    let (app, _, shared) = app_with_mock_mineru(MockMineruMode::ProgressThenDone).await;

    // Simulate a crash after upload: the row claims pending but no task was
    // ever spawned (the usual post-restart state).
    let id = create_article(&app, "Interrupted", "10.1000/interrupted", 2024).await;
    let bytes = b"%PDF-1.4 interrupted upload".to_vec();
    let stored = bib_base::stored_fulltext(&id, "interrupted.pdf", &bytes);
    let path = bib_base::vfs_virtual_path(&stored.path).unwrap();
    shared
        .file_storage
        .as_ref()
        .unwrap()
        .write_bytes(&path, bytes)
        .await
        .unwrap();
    shared
        .bib
        .upsert_fulltext(&bib_types::FullText {
            article_id: id.clone(),
            file_path: stored.path,
            file_format: bib_types::FileFormat::Pdf,
            text_content: None,
            source: bib_types::FullTextSource::UserUpload,
            file_hash: Some(stored.file_hash),
            file_size: None,
            uploaded_at: None,
            parse_status: "pending".to_owned(),
            parse_engine: None,
            parse_error: None,
        })
        .await
        .unwrap();

    // Opening the stream lazily re-spawns the parse, which now runs to
    // completion against the mock.
    let response = app
        .clone()
        .oneshot(request(
            "GET",
            &format!("/api/v1/bib/articles/{}/parse/stream", encode_id(&id)),
            None,
        ))
        .await
        .unwrap();
    let stream_bytes = response.into_body().collect().await.unwrap().to_bytes();
    let stream = String::from_utf8_lossy(&stream_bytes);
    assert!(stream.contains("event: done"), "stream: {stream}");

    let (status, body) = send(
        &app,
        "GET",
        &format!("/api/v1/bib/articles/{}/fulltext", encode_id(&id)),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body["fulltext"]["text_content"]
        .as_str()
        .unwrap()
        .contains("Mock MinerU Title"));
}

#[tokio::test]
async fn reparse_validates_its_target_and_conflicts_with_a_running_parse() {
    let (app, _, shared) = app_with_mock_mineru(MockMineruMode::NeverDone).await;

    // No full text at all → 404.
    let id = create_article(&app, "Reparse Target", "10.1000/reparse", 2024).await;
    let encoded = encode_id(&id);
    let (status, body) = send(&app, "POST", &format!("/api/v1/bib/articles/{encoded}/reparse"), None)
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "body: {body}");

    // A full text without a VFS object (open-access fetch shape) → 400.
    shared
        .bib
        .upsert_fulltext(&bib_types::FullText {
            article_id: id.clone(),
            file_path: "europepmc:PMC123456".to_owned(),
            file_format: bib_types::FileFormat::Txt,
            text_content: Some("open-access text".to_owned()),
            source: bib_types::FullTextSource::OpenAccess,
            file_hash: None,
            file_size: None,
            uploaded_at: None,
            parse_status: "done".to_owned(),
            parse_engine: None,
            parse_error: None,
        })
        .await
        .unwrap();
    let (status, body) = send(&app, "POST", &format!("/api/v1/bib/articles/{encoded}/reparse"), None)
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "body: {body}");
    assert!(body["error"].as_str().unwrap().contains("VFS"));

    // Replace with a real stored PDF: the parse starts and stays running
    // (NeverDone), so a concurrent reparse must answer 409.
    let (status, body) = send_multipart_uri(
        &app,
        &format!("/api/v1/bib/articles/{encoded}/fulltext"),
        "scan.pdf",
        "%PDF-1.4 scan bytes",
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    assert_eq!(body["fulltext"]["parse_status"], "pending");

    let (status, body) = send(&app, "POST", &format!("/api/v1/bib/articles/{encoded}/reparse"), None)
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "body: {body}");
}

#[tokio::test]
async fn txt_uploads_stay_synchronous() {
    let (app, _, _) = app_with_mock_mineru(MockMineruMode::ProgressThenDone).await;

    // Even with a live MinerU mock, a txt upload is readable immediately.
    let (status, body) =
        send_multipart(&app, "notes.txt", "Plain prose, no identifiers.", &[]).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["fulltext"]["parse_status"], "done");
    assert_eq!(body["fulltext"]["parse_engine"], "builtin");
    assert_eq!(body["text_chars"], 28);

    let id = body["article"]["id"].as_str().unwrap();
    let (status, body) = send(
        &app,
        "GET",
        &format!("/api/v1/bib/fulltext-statuses"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let entry = body["statuses"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["article_id"].as_str() == Some(id))
        .unwrap();
    assert_eq!(entry["parse_status"], "done");
    assert_eq!(entry["parse_engine"], "builtin");
}
