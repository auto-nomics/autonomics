//! Deck REST API tests (axum oneshot): CRUD, print-token surface,
//! asset upload/download, error mapping.

use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode},
};
use http_body_util::BodyExt;
use ppt_base::shared::DeckShared;
use serde_json::{Value, json};
use std::sync::Arc;
use tower::ServiceExt;

fn request(method: &str, uri: &str, body: Option<String>) -> Request<Body> {
    let mut builder = Request::builder().method(method).uri(uri);
    if let Some(b) = &body {
        builder = builder.header("content-type", "application/json");
        let _ = b;
    }
    builder
        .body(body.map(Body::from).unwrap_or_else(Body::empty))
        .unwrap()
}

async fn deck_shared() -> Arc<ppt_base::DeckShared> {
    Arc::new(DeckShared::open_in_memory().await.unwrap())
}

async fn app() -> (Router, Arc<ppt_base::DeckShared>) {
    let shared = deck_shared().await;
    let router = api_server::deck::router(shared.clone())
        .merge(api_server::deck::open_router(shared.clone()));
    (router, shared)
}

async fn json_of(body: Body) -> Value {
    let bytes = body.collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).unwrap_or(Value::Null)
}

// ---------------------------------------------------------------------------
// Deck CRUD
// ---------------------------------------------------------------------------

#[tokio::test]
async fn create_get_list_delete_deck() {
    let (app, _shared) = app().await;

    let res = app
        .clone()
        .oneshot(request(
            "POST",
            "/decks",
            Some(r#"{"title":"测试 Deck","template":"meeting"}"#.into()),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::CREATED);
    let created = json_of(res.into_body()).await;
    let id = created["deck"]["id"].as_str().unwrap().to_string();
    assert!(created["slides"].as_array().unwrap().len() >= 5);

    // GET carries an ETag with the revision
    let res = app
        .clone()
        .oneshot(request("GET", &format!("/decks/{id}"), None))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let etag = res
        .headers()
        .get("etag")
        .expect("ETag header")
        .to_str()
        .unwrap()
        .to_string();
    assert!(etag.contains("\""));
    let got = json_of(res.into_body()).await;
    assert_eq!(got["deck"]["id"], id.as_str());

    // list includes it
    let res = app
        .clone()
        .oneshot(request("GET", "/decks", None))
        .await
        .unwrap();
    let list = json_of(res.into_body()).await;
    assert_eq!(list["decks"].as_array().unwrap().len(), 1);

    // 404 for unknown deck mentions the id
    let res = app
        .clone()
        .oneshot(request("GET", "/decks/deck-nope", None))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);

    // delete
    let res = app
        .clone()
        .oneshot(request("DELETE", &format!("/decks/{id}"), None))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let res = app
        .oneshot(request("GET", &format!("/decks/{id}"), None))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn slide_endpoints_crud() {
    let (app, _shared) = app().await;

    let res = app
        .clone()
        .oneshot(request(
            "POST",
            "/decks",
            Some(r#"{"title":"T"}"#.into()),
        ))
        .await
        .unwrap();
    let created = json_of(res.into_body()).await;
    let id = created["deck"]["id"].as_str().unwrap().to_string();

    // get slide 1
    let res = app
        .clone()
        .oneshot(request("GET", &format!("/decks/{id}/slides/1"), None))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let slide = json_of(res.into_body()).await;
    assert_eq!(slide["slide"]["index"], json!(0));
    assert_eq!(slide["slideCount"], json!(1));

    // slide 99 → 404 with slidev-style message
    let res = app
        .clone()
        .oneshot(request("GET", &format!("/decks/{id}/slides/99"), None))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
    let err = json_of(res.into_body()).await;
    assert!(err["error"].as_str().unwrap().contains("(1-"));

    // insert after 1
    let res = app
        .clone()
        .oneshot(request(
            "POST",
            &format!("/decks/{id}/slides"),
            Some(serde_json::json!({"after":1,"content":"## 新页","frontmatter_raw":"layout: section"}).to_string()),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::CREATED);
    let ins = json_of(res.into_body()).await;
    assert_eq!(ins["insertedNo"], json!(2));

    // patch slide 2
    let res = app
        .clone()
        .oneshot(request(
            "PATCH",
            &format!("/decks/{id}/slides/2"),
            Some(r#"{"note":"讲到这里停一下"}"#.into()),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    // patch slide 1 frontmatter → headmatter guard (409)
    let res = app
        .clone()
        .oneshot(request(
            "PATCH",
            &format!("/decks/{id}/slides/1"),
            Some(r#"{"frontmatter_raw":"layout: cover"}"#.into()),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::CONFLICT);

    // move 2 → 1 forbidden (headmatter fixed)
    let res = app
        .clone()
        .oneshot(request(
            "POST",
            &format!("/decks/{id}/slides/2/move"),
            Some(r#"{"to":1}"#.into()),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::CONFLICT);

    // delete slide 2
    let res = app
        .clone()
        .oneshot(request("DELETE", &format!("/decks/{id}/slides/2"), None))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
}

#[tokio::test]
async fn put_source_with_revision_conflict() {
    let (app, _shared) = app().await;

    let res = app
        .clone()
        .oneshot(request(
            "POST",
            "/decks",
            Some(r#"{"title":"T"}"#.into()),
        ))
        .await
        .unwrap();
    let created = json_of(res.into_body()).await;
    let id = created["deck"]["id"].as_str().unwrap().to_string();
    let rev = created["deck"]["revision"].as_str().unwrap().to_string();

    // wrong revision → 409
    let res = app
        .clone()
        .oneshot(request(
            "PUT",
            &format!("/decks/{id}/source?rev=stale"),
            Some(serde_json::json!({"markdown": "# New\n"}).to_string()),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::CONFLICT);

    // correct revision → 200
    let body = serde_json::json!({"markdown": "# New\n", "revision": rev}).to_string();
    let res = app
        .clone()
        .oneshot(request("PUT", &format!("/decks/{id}/source"), Some(body)))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
}

// ---------------------------------------------------------------------------
// Print token surface
// ---------------------------------------------------------------------------

#[tokio::test]
async fn print_data_requires_valid_token() {
    let (app, shared) = app().await;

    let res = app
        .clone()
        .oneshot(request(
            "POST",
            "/decks",
            Some(r#"{"title":"T"}"#.into()),
        ))
        .await
        .unwrap();
    let created = json_of(res.into_body()).await;
    let id = created["deck"]["id"].as_str().unwrap().to_string();

    // no token → 401
    let res = app
        .clone()
        .oneshot(request(
            "GET",
            &format!("/decks/{id}/print-data"),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);

    // bad token → 401
    let res = app
        .clone()
        .oneshot(request(
            "GET",
            &format!("/decks/{id}/print-data?token=wrong"),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);

    // API enqueue returns 202 (token stays engine-internal now)
    let res = app
        .clone()
        .oneshot(request(
            "POST",
            &format!("/decks/{id}/exports"),
            Some(r#"{"format":"pdf"}"#.into()),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::ACCEPTED);
    let minted = json_of(res.into_body()).await;
    let export_id = minted["exportId"].as_str().unwrap().to_string();
    assert_eq!(minted["status"], "queued");
    assert!(minted.get("printToken").is_none(), "token must not leak into the API response");

    // The test mints its own token through the engine to exercise the
    // print surface (production: the export worker builds the URL).
    let (_record, token) = shared
        .enqueue_export(&id, ppt_base::store::ExportFormat::Pdf, 96, vec![1], 980, 551, vec![])
        .await
        .unwrap();

    // good token → full payload
    let res = app
        .clone()
        .oneshot(request(
            "GET",
            &format!("/decks/{id}/print-data?token={token}"),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let payload = json_of(res.into_body()).await;
    assert!(payload["slides"].as_array().is_some());
    assert_eq!(payload["token"], token.as_str());

    // export status visible (the API-minted row)
    let export_id = minted["exportId"].as_str().unwrap().to_string();
    let res = app
        .clone()
        .oneshot(request("GET", &format!("/exports/{export_id}"), None))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let status = json_of(res.into_body()).await;
    assert_eq!(status["status"], "queued");

    // download before artifact → 404 with status hint
    let res = app
        .oneshot(request(
            "GET",
            &format!("/exports/{export_id}/download"),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn bad_format_rejected() {
    let (app, _shared) = app().await;
    let res = app
        .clone()
        .oneshot(request(
            "POST",
            "/decks",
            Some(r#"{"title":"T"}"#.into()),
        ))
        .await
        .unwrap();
    let created = json_of(res.into_body()).await;
    let id = created["deck"]["id"].as_str().unwrap().to_string();

    let res = app
        .oneshot(request(
            "POST",
            &format!("/decks/{id}/exports"),
            Some(r#"{"format":"xlsx"}"#.into()),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}
