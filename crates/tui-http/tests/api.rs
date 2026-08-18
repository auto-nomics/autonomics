use axum::body::Body;
use axum::http::{Request, StatusCode};
use bib_base::BibShared;
use http_body_util::BodyExt;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tower::ServiceExt;

fn request(method: &str, uri: &str, body: Option<String>) -> Request<Body> {
    let mut builder = Request::builder().method(method).uri(uri);
    if body.is_some() {
        builder = builder.header("content-type", "application/json");
    }
    builder
        .body(body.map(Body::from).unwrap_or_else(Body::empty))
        .unwrap()
}

#[tokio::test]
async fn server_serves_requests_and_shuts_down_gracefully() {
    let shared = BibShared::open_in_memory().await.unwrap();
    let server = tui_http::start(tui_http::api_router(shared), "127.0.0.1:0")
        .await
        .unwrap();

    let mut stream = TcpStream::connect(server.addr()).await.unwrap();
    stream
        .write_all(b"GET /api/health HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();
    let mut response = Vec::new();
    stream.read_to_end(&mut response).await.unwrap();
    let response = String::from_utf8_lossy(&response);
    assert!(response.starts_with("HTTP/1.1 200 OK\r\n"));
    assert!(response.contains(r#"{"status":"ok"}"#));

    server.shutdown().await.unwrap();
}

#[tokio::test]
async fn aggregate_router_exposes_bib_module() {
    let shared = BibShared::open_in_memory().await.unwrap();
    let app = tui_http::api_router(shared);

    let response = app
        .clone()
        .oneshot(request("GET", "/api/v1", None))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(body["modules"][0], "bib");

    let response = app
        .oneshot(request("GET", "/api/health", None))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn bib_collections_and_annotations_use_nested_routes() {
    let shared = BibShared::open_in_memory().await.unwrap();
    let app = tui_http::api_router(shared);

    let response = app
        .clone()
        .oneshot(request(
            "POST",
            "/api/v1/bib/collections",
            Some(r#"{ "name": "Reading list" }"#.to_owned()),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let collection_id = body["collection"]["id"].as_str().unwrap().to_owned();

    let response = app
        .clone()
        .oneshot(request(
            "POST",
            "/api/v1/bib/articles",
            Some(
                r#"{
                    "title": "Nested collection test",
                    "doi": "10.1000/nested-collection-test"
                }"#
                .to_owned(),
            ),
        ))
        .await
        .unwrap();
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let article_id = body["article"]["id"].as_str().unwrap().to_owned();

    let response = app
        .clone()
        .oneshot(request(
            "POST",
            &format!("/api/v1/bib/collections/{collection_id}/articles"),
            Some(serde_json::json!({ "article_id": article_id }).to_string()),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let encoded_article_id = article_id.replace('/', "%2F");
    let response = app
        .oneshot(request(
            "POST",
            &format!("/api/v1/bib/articles/{encoded_article_id}/annotations"),
            Some(r#"{ "content": "Useful for the API test" }"#.to_owned()),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
}

#[tokio::test]
async fn bib_article_crud_uses_nested_routes() {
    let shared = BibShared::open_in_memory().await.unwrap();
    let app = tui_http::api_router(shared);
    let payload = r#"{
        "title": "Local API test",
        "doi": "10.1000/local-api-test",
        "authors": ["Smith John"],
        "year": 2026,
        "source": "manual"
    }"#;

    let response = app
        .clone()
        .oneshot(request(
            "POST",
            "/api/v1/bib/articles",
            Some(payload.to_owned()),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(body["article"]["title"], "Local API test");

    let response = app
        .oneshot(request(
            "GET",
            "/api/v1/bib/articles/doi%3A10.1000%2Flocal-api-test",
            None,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}
