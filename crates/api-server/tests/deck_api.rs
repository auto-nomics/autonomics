//! Deck REST API tests (axum oneshot): CRUD, print-token surface,
//! asset upload/download, error mapping.

use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode},
};
use http_body_util::BodyExt;
use ppt_base::DeckHandle;
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

async fn deck_handle() -> Arc<DeckHandle> {
    Arc::new(DeckHandle::spawn(Arc::new(DeckShared::open_in_memory().await.unwrap())))
}

async fn app() -> (Router, Arc<DeckHandle>) {
    let handle = deck_handle().await;
    let router = api_server::deck::router(handle.clone())
        .merge(api_server::deck::open_router(handle.clone()));
    (router, handle)
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
        .shared()
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

#[tokio::test]
async fn theme_and_slide_css_are_independent_versioned_and_follow_pages() {
    let (app, handle) = app().await;
    let markdown = "---\ntitle: T\n---\n\n# Cover\n\n---\nhide: true\n---\n\n# Hidden\n\n---\n\n# First\n\n---\n\n# Second\n";
    let response = app.clone().oneshot(request("POST", "/decks", Some(json!({"title":"T", "markdown":markdown}).to_string()))).await.unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
    let created = json_of(response.into_body()).await;
    let id = created["deck"]["id"].as_str().unwrap();
    let initial_rev = created["deck"]["revision"].as_str().unwrap();
    let theme_rev = created["themeRevision"].as_str().unwrap();
    let theme = ":root { --deck-width: 1280px; --deck-height: 720px; } :scope { background: rgb(1, 2, 3); }";
    let response = app.clone().oneshot(request("PUT", &format!("/decks/{id}/theme.css"), Some(json!({"css":theme,"revision":theme_rev}).to_string()))).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let response = app.clone().oneshot(request("PUT", &format!("/decks/{id}/theme.css"), Some(json!({"css":"stale", "revision":theme_rev}).to_string()))).await.unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);
    let response = app.clone().oneshot(request("GET", &format!("/decks/{id}"), None)).await.unwrap();
    let current = json_of(response.into_body()).await;
    assert_eq!(current["themeCss"], theme);
    assert_eq!(current["canvasSize"], json!({"width":1280,"height":720}));
    assert_eq!(current["deck"]["revision"], initial_rev);
    let invalid = app.clone().oneshot(request("PUT", &format!("/decks/{id}/theme.css"), Some(json!({"css":":root { --deck-width: 1280px; }", "revision":current["themeRevision"]}).to_string()))).await.unwrap();
    assert_eq!(invalid.status(), StatusCode::BAD_REQUEST);
    let export = app.clone().oneshot(request("POST", &format!("/decks/{id}/exports"), Some(json!({"format":"pptx"}).to_string()))).await.unwrap();
    assert_eq!(export.status(), StatusCode::ACCEPTED);
    let exports = handle.shared().store.list_exports(id).await.unwrap();
    let params: Value = serde_json::from_str(&exports[0].params).unwrap();
    assert_eq!((params["canvasWidth"].as_u64(), params["canvasHeight"].as_u64()), (Some(1280), Some(720)));

    let css = ".deck-prose h1 { color: red; } /* --- */";
    let response = app.clone().oneshot(request("PUT", &format!("/decks/{id}/slides/2/css"), Some(json!({"css":css,"revision":initial_rev}).to_string()))).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let slide_rev = json_of(response.into_body()).await["revision"].as_str().unwrap().to_owned();
    let response = app.clone().oneshot(request("PUT", &format!("/decks/{id}/slides/2/css"), Some(json!({"css":"stale", "revision":initial_rev}).to_string()))).await.unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);
    let response = app.clone().oneshot(request("GET", &format!("/decks/{id}"), None)).await.unwrap();
    let current = json_of(response.into_body()).await;
    assert_eq!(current["slides"][1]["title"], "First");
    assert_eq!(current["slides"][1]["frontmatter"]["slideCss"], css);
    assert_eq!(current["slides"][2]["title"], "Second");
    assert_eq!(current["deck"]["revision"], slide_rev);

    // Source-level page move: the hidden page occupies position 2.
    let response = app.clone().oneshot(request("POST", &format!("/decks/{id}/slides/3/move"), Some(json!({"to":4}).to_string()))).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let response = app.clone().oneshot(request("GET", &format!("/decks/{id}"), None)).await.unwrap();
    let moved = json_of(response.into_body()).await;
    assert_eq!(moved["slides"][2]["title"], "First");
    assert_eq!(moved["slides"][2]["frontmatter"]["slideCss"], css);

    let size = handle.shared().canvas_size(id, &handle.shared().load_deck(id).await.unwrap().meta).await.unwrap();
    let (_, token) = handle.shared().enqueue_export(id, ppt_base::store::ExportFormat::Pdf, 96, vec![1], size.width, size.height, vec![]).await.unwrap();
    let current_theme = app.clone().oneshot(request("GET", &format!("/decks/{id}/theme.css"), None)).await.unwrap();
    let theme_revision = json_of(current_theme.into_body()).await["revision"].as_str().unwrap().to_owned();
    let updated = app.clone().oneshot(request("PUT", &format!("/decks/{id}/theme.css"), Some(json!({"css":":root { --deck-width: 1920px; --deck-height: 1080px; }", "revision":theme_revision}).to_string()))).await.unwrap();
    assert_eq!(updated.status(), StatusCode::OK);
    let default_theme = app.clone().oneshot(request("GET", &format!("/decks/{id}/default-theme.css"), None)).await.unwrap();
    let default_css = json_of(default_theme.into_body()).await["css"].as_str().unwrap().to_owned();
    assert!(default_css.contains("--deck-width: 1920px"));
    assert!(default_css.contains("--deck-height: 1080px"));
    assert!(default_css.contains("--bio-accent"));
    let response = app.clone().oneshot(request("GET", &format!("/decks/{id}/print-data?token={token}"), None)).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let printed = json_of(response.into_body()).await;
    assert_eq!(printed["canvasSize"], json!({"width":1280,"height":720}));
    assert_eq!(printed["themeCss"], theme);
    assert_eq!(printed["slides"][2]["frontmatter"]["slideCss"], css);
    let response = app.clone().oneshot(request("DELETE", &format!("/decks/{id}"), None)).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(handle.shared().assets.get_optional(&format!("themes/{id}/theme.css")).await.unwrap().is_none());
}

// ---------------------------------------------------------------------------
// Diagnostics
// ---------------------------------------------------------------------------

#[tokio::test]
async fn diagnostics_post_runs_and_get_serves_report() {
    let (app, handle) = app().await;
    let res = app
        .clone()
        .oneshot(request("POST", "/decks", Some(r#"{"title":"诊断","template":"meeting"}"#.into())))
        .await
        .unwrap();
    let created = json_of(res.into_body()).await;
    let id = created["deck"]["id"].as_str().unwrap().to_string();

    // No report yet: GET reports null + fresh=false.
    let res = app.clone().oneshot(request("GET", &format!("/decks/{id}/diagnostics"), None)).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let state = json_of(res.into_body()).await;
    assert_eq!(state["report"], Value::Null);
    assert_eq!(state["fresh"], json!(false));

    // POST forces a synchronous run. NullExporter in tests → static tier
    // only, rendered=false; the meeting template has no empty slides, so
    // findings may legitimately be empty.
    let res = app.clone().oneshot(request("POST", &format!("/decks/{id}/diagnostics"), None)).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let report = json_of(res.into_body()).await;
    assert_eq!(report["deckId"], json!(id));
    assert_eq!(report["rendered"], json!(false));

    // GET now serves the stored report, fresh against current revisions.
    let res = app.clone().oneshot(request("GET", &format!("/decks/{id}/diagnostics"), None)).await.unwrap();
    let state = json_of(res.into_body()).await;
    assert_eq!(state["fresh"], json!(true));
    assert_eq!(state["report"]["revision"], report["revision"]);

    // Deck payload carries the summary badge.
    let res = app.clone().oneshot(request("GET", &format!("/decks/{id}"), None)).await.unwrap();
    let payload = json_of(res.into_body()).await;
    assert_eq!(payload["diagnostics"]["fresh"], json!(true));
    assert_eq!(payload["diagnostics"]["rendered"], json!(false));

    // Deleting the deck drops the report.
    let res = app.clone().oneshot(request("DELETE", &format!("/decks/{id}"), None)).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert!(handle.shared().latest_diagnostics(&id).await.unwrap().is_none());
}

#[tokio::test]
async fn outline_matches_between_viewer_and_print_and_tracks_edits() {
    let (app, handle) = app().await;
    let markdown = "# Cover\n\n---\nlayout: section\nsectionTitle: 方法\n---\n\n# 研究方法\n\n---\n\n## 队列\n\n---\nlayout: section\nlevel: 2\n---\n\n## 亚组\n\n---\nhideInToc: true\n---\n\n### 隐藏页面\n";
    let response = app.clone().oneshot(request("POST", "/decks", Some(json!({"title":"Outline", "markdown":markdown}).to_string()))).await.unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
    let created = json_of(response.into_body()).await;
    let id = created["deck"]["id"].as_str().unwrap();
    assert_eq!(created["outline"][1]["sectionTitle"], "方法");
    assert_eq!(created["outline"][2]["parent"], 2);
    assert_eq!(created["outline"][4]["sectionPath"], json!([2,4]));
    assert_eq!(created["outline"][4]["hidden"], true);
    let (_, token) = handle.shared().enqueue_export(id, ppt_base::store::ExportFormat::Pdf, 96, vec![1], 980, 551, vec![]).await.unwrap();
    let response = app.clone().oneshot(request("GET", &format!("/decks/{id}/print-data?token={token}"), None)).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(json_of(response.into_body()).await["outline"], created["outline"]);
    let response = app.clone().oneshot(request("POST", &format!("/decks/{id}/slides/2/move"), Some(json!({"to":4}).to_string()))).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let response = app.oneshot(request("GET", &format!("/decks/{id}"), None)).await.unwrap();
    let updated = json_of(response.into_body()).await;
    assert_eq!(updated["outline"][3]["sectionTitle"], "方法");
    assert_eq!(updated["outline"][3]["slide"], 4);
}
