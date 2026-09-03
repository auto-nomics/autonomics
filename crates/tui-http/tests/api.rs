use axum::body::Body;
use axum::http::{Request, StatusCode};
use bib_base::BibShared;
use http_body_util::BodyExt;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tower::ServiceExt;
use vfs::{
    BackendConfig, BackendDefinition, MountDefinition, MountedObjectStore, OpendalFileStorage,
    VfsManifest,
};

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
async fn server_serves_the_bibliography_frontend() {
    let shared = BibShared::open_in_memory().await.unwrap();
    let app = tui_http::api_router(shared);

    // The SPA shell (rust-embed from apps/web/dist) with its hashed assets.
    for (path, marker) in [("/", "<title>Autonomics</title>"), ("/paper/abc", "<div id=\"root\">")] {
        let response = app
            .clone()
            .oneshot(request("GET", path, None))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK, "failed path: {path}");
        let body = response.into_body().collect().await.unwrap().to_bytes();
        let body = String::from_utf8_lossy(&body).to_string();
        assert!(body.contains(marker), "missing marker for {path}");
    }

    // The PDFium WASM engine must arrive with the correct MIME type or the
    // browser refuses to instantiate it.
    let response = app
        .clone()
        .oneshot(request("GET", "/wasm/pdfium.wasm", None))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers().get("content-type").unwrap(),
        "application/wasm"
    );

    // Unmatched API routes stay JSON 404s even though the SPA fallback sees
    // every unrouted request.
    let response = app
        .oneshot(request("GET", "/api/v1/bib/nope", None))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(body["error"].is_string());
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
        .clone()
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
        .clone()
        .oneshot(request(
            "GET",
            "/api/v1/bib/articles/doi%3A10.1000%2Flocal-api-test",
            None,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn bib_upload_preserves_and_serves_the_original_file() {
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
    let file_storage = Arc::new(OpendalFileStorage::with_mounts(
        directory.path(),
        vfs.clone(),
    ));
    let shared = BibShared::open_in_memory()
        .await
        .unwrap()
        .with_file_storage(file_storage);
    let app = tui_http::api_router(shared);

    let response = app
        .clone()
        .oneshot(request(
            "POST",
            "/api/v1/bib/articles",
            Some(r#"{ "title": "Original upload", "doi": "10.1000/original-upload" }"#.to_owned()),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);

    let content = b"original full-text bytes";
    let multipart = format!(
        "--boundary\r\ncontent-disposition: form-data; name=\"file\"; filename=\"original.txt\"\r\ncontent-type: text/plain\r\n\r\n{}\r\n--boundary--\r\n",
        String::from_utf8_lossy(content)
    );
    let upload_request = Request::builder()
        .method("POST")
        .uri("/api/v1/bib/articles/doi%3A10.1000%2Foriginal-upload/fulltext")
        .header("content-type", "multipart/form-data; boundary=boundary")
        .body(Body::from(multipart))
        .unwrap();
    let response = app.clone().oneshot(upload_request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let fulltext = &body["fulltext"];
    let file_path = fulltext["file_path"].as_str().unwrap();
    assert!(file_path.starts_with("vfs:///literature/doi%3A10.1000%2Foriginal-upload/"));
    assert!(file_path.ends_with("-original.txt"));
    assert_eq!(fulltext["file_size"], content.len() as i64);
    assert!(
        fulltext["file_hash"]
            .as_str()
            .is_some_and(|hash| hash.len() == 64)
    );

    let response = app
        .clone()
        .oneshot(request(
            "GET",
            "/api/v1/bib/articles/doi%3A10.1000%2Foriginal-upload/fulltext/raw",
            None,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers().get("content-type").unwrap(),
        "text/plain; charset=utf-8"
    );
    assert!(
        response
            .headers()
            .get("content-disposition")
            .unwrap()
            .to_str()
            .unwrap()
            .starts_with("attachment;")
    );
    let body = response.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(&body[..], content);

    let response = app
        .clone()
        .oneshot(request(
            "GET",
            "/api/v1/bib/articles/doi%3A10.1000%2Foriginal-upload/fulltext?offset=9&limit=6",
            None,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(body["fulltext"]["text_content"], "full-t");
    assert_eq!(body["pagination"]["offset"], 9);
    assert_eq!(body["pagination"]["limit"], 6);
    assert_eq!(body["pagination"]["total_chars"], 24);
    assert_eq!(body["pagination"]["next_offset"], 15);

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/api/v1/bib/articles/doi%3A10.1000%2Foriginal-upload/fulltext/raw")
                .header("range", "bytes=9-14")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(
        response.headers().get("content-range").unwrap(),
        "bytes 9-14/24"
    );
    let body = response.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(&body[..], b"full-t");

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("HEAD")
                .uri("/api/v1/bib/articles/doi%3A10.1000%2Foriginal-upload/fulltext/raw")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers().get("content-length").unwrap(), "24");

    let response = app
        .clone()
        .oneshot(request(
            "DELETE",
            "/api/v1/bib/articles/doi%3A10.1000%2Foriginal-upload/fulltext",
            None,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let response = app
        .oneshot(request(
            "GET",
            "/api/v1/bib/articles/doi%3A10.1000%2Foriginal-upload/fulltext/raw",
            None,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn bib_upload_stores_original_pdf_bytes_with_application_pdf_mime() {
    let (app, _directory) = build_app_with_vfs().await;

    let response = app
        .clone()
        .oneshot(request(
            "POST",
            "/api/v1/bib/articles",
            Some(r#"{ "title": "PDF hosting test", "doi": "10.1000/pdf-host" }"#.to_owned()),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);

    let pdf_bytes: &[u8] = b"%PDF-1.4\n%placeholder original PDF bytes\n%%EOF\n";
    let multipart = format!(
        "--boundary\r\ncontent-disposition: form-data; name=\"file\"; filename=\"paper.pdf\"\r\ncontent-type: application/pdf\r\n\r\n{}\r\n--boundary--\r\n",
        String::from_utf8_lossy(pdf_bytes)
    );
    let upload_request = Request::builder()
        .method("POST")
        .uri("/api/v1/bib/articles/doi%3A10.1000%2Fpdf-host/fulltext")
        .header("content-type", "multipart/form-data; boundary=boundary")
        .body(Body::from(multipart))
        .unwrap();
    let response = app.clone().oneshot(upload_request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(body["fulltext"]["file_format"], "pdf");
    let file_path = body["fulltext"]["file_path"].as_str().unwrap();
    assert!(file_path.starts_with("vfs:///literature/doi%3A10.1000%2Fpdf-host/"));
    assert!(file_path.ends_with("-paper.pdf"));
    assert_eq!(body["fulltext"]["file_size"], pdf_bytes.len() as i64);

    let response = app
        .clone()
        .oneshot(request(
            "GET",
            "/api/v1/bib/articles/doi%3A10.1000%2Fpdf-host/fulltext/raw",
            None,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers().get("content-type").unwrap(),
        "application/pdf"
    );
    let body = response.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(&body[..], pdf_bytes);
}

#[tokio::test]
async fn api_bearer_auth_protects_api_routes_only() {
    let shared = BibShared::open_in_memory().await.unwrap();
    let app = tui_http::api_router_with_auth(shared, Some("test-token".to_owned()));

    let response = app
        .clone()
        .oneshot(request("GET", "/api/health", None))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/health")
                .header("authorization", "Bearer test-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let response = app.oneshot(request("GET", "/", None)).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

#[test]
fn http_api_defaults_to_localhost() {
    assert_eq!(tui_http::DEFAULT_HTTP_API_ADDR, "127.0.0.1:8765");
}

async fn build_app_with_vfs() -> (axum::Router, tempfile::TempDir) {
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
    let file_storage = Arc::new(OpendalFileStorage::with_mounts(
        directory.path(),
        vfs.clone(),
    ));
    let shared = BibShared::open_in_memory()
        .await
        .unwrap()
        .with_file_storage(file_storage);
    (tui_http::api_router(shared), directory)
}

// ── agent chat (SSE) ───────────────────────────────────────────────────────

/// Scripted LLM client: fails every request with a *non-retryable* 4xx so
/// the turn terminates immediately (agentik would otherwise burn its retry
/// budget on retryable errors and slow the test down).
struct FailingClient;

#[async_trait::async_trait]
impl agentik_sdk::provider::client::ApiClient for FailingClient {
    async fn request(
        &self,
        _messages: Vec<agentik_types::Message>,
        _tools: Vec<agentik_types::ToolDefinition>,
        _model_info: &agentik_sdk::model::ModelInfo,
    ) -> Result<agentik_types::Message, agentik_types::errors::AnthropicError> {
        Err(agentik_types::errors::AnthropicError::HttpError {
            status: 400,
            message: "scripted failure".to_owned(),
        })
    }

    async fn request_stream(
        &self,
        _messages: Vec<agentik_types::Message>,
        _tools: Vec<agentik_types::ToolDefinition>,
        _model_info: &agentik_sdk::model::ModelInfo,
    ) -> Result<agentik_sdk::streaming::MessageStream, agentik_types::errors::AnthropicError>
    {
        Err(agentik_types::errors::AnthropicError::HttpError {
            status: 400,
            message: "scripted failure".to_owned(),
        })
    }

    async fn test_connection(&self) -> Result<(), agentik_types::errors::AnthropicError> {
        Ok(())
    }
}

fn chat_body(message: &str) -> Option<String> {
    Some(format!(r#"{{"message": "{message}"}}"#))
}

#[tokio::test]
async fn agent_chat_is_absent_without_a_model_slot() {
    let shared = BibShared::open_in_memory().await.unwrap();
    let app = tui_http::api_router(shared);

    let response = app
        .oneshot(request("POST", "/api/v1/agent/chat", chat_body("hi")))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn agent_chat_returns_503_when_no_model_is_configured() {
    let shared = BibShared::open_in_memory().await.unwrap();
    let app = tui_http::ApiRouterBuilder::new(shared)
        .model(Arc::new(arc_swap::ArcSwapOption::default()))
        .build();

    let response = app
        .oneshot(request("POST", "/api/v1/agent/chat", chat_body("hi")))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(body["error"].is_string());
}

#[tokio::test]
async fn agent_chat_rejects_an_empty_message() {
    let shared = BibShared::open_in_memory().await.unwrap();
    let model = agentik_sdk::model::Model::with_client(
        agentik_core::testing::dummy_model_info("test-model"),
        FailingClient,
    );
    let app = tui_http::ApiRouterBuilder::new(shared)
        .model(Arc::new(arc_swap::ArcSwapOption::from(Some(Arc::new(
            model,
        )))))
        .build();

    let response = app
        .clone()
        .oneshot(request("POST", "/api/v1/agent/chat", chat_body("   ")))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn agent_chat_streams_model_errors_as_sse_events() {
    let shared = BibShared::open_in_memory().await.unwrap();
    let model = agentik_sdk::model::Model::with_client(
        agentik_core::testing::dummy_model_info("test-model"),
        FailingClient,
    );
    let app = tui_http::ApiRouterBuilder::new(shared)
        .model(Arc::new(arc_swap::ArcSwapOption::from(Some(Arc::new(
            model,
        )))))
        .build();

    let response = app
        .oneshot(request("POST", "/api/v1/agent/chat", chat_body("hi")))
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

    let body = response.into_body().collect().await.unwrap().to_bytes();
    let body = String::from_utf8_lossy(&body);
    assert!(body.contains("event: error"), "stream: {body}");
    assert!(body.contains("scripted failure"), "stream: {body}");
}
