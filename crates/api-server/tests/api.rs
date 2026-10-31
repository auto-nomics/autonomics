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

#[tokio::test]
async fn server_serves_requests_and_shuts_down_gracefully() {
    let shared = BibShared::open_in_memory().await.unwrap();
    let server = api_server::start(
        api_server::api_router(shared.clone(), wiki_handle().await),
        "127.0.0.1:0",
    )
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
    let app = api_server::api_router(shared.clone(), wiki_handle().await);

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
    let app = api_server::api_router(shared.clone(), wiki_handle().await);

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
    let app = api_server::api_router(shared.clone(), wiki_handle().await);
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
            permissions: Default::default(),
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
    let app = api_server::api_router(shared.clone(), wiki_handle().await);

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
    // Uploads return immediately; extraction runs in the background.
    assert_eq!(fulltext["extract_status"], "pending");
    assert!(fulltext["text_content"].is_null());

    // The upload spawned a background extraction; wait for it to land
    // before asserting on extracted text.
    let body = wait_for_extraction(
        &app,
        "/api/v1/bib/articles/doi%3A10.1000%2Foriginal-upload/fulltext",
    )
    .await;
    assert_eq!(body["fulltext"]["text_content"], "original full-text bytes");

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
async fn bib_reextract_resets_and_rewrites_the_text() {
    let (app, _shared, _directory) = build_app_with_vfs().await;

    let response = app
        .clone()
        .oneshot(request(
            "POST",
            "/api/v1/bib/articles",
            Some(r#"{ "title": "Re-extract test", "doi": "10.1000/reextract-test" }"#.to_owned()),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);

    let content = b"re-extract these bytes";
    let multipart = format!(
        "--boundary\r\ncontent-disposition: form-data; name=\"file\"; filename=\"note.txt\"\r\ncontent-type: text/plain\r\n\r\n{}\r\n--boundary--\r\n",
        String::from_utf8_lossy(content)
    );
    let upload_request = Request::builder()
        .method("POST")
        .uri("/api/v1/bib/articles/doi%3A10.1000%2Freextract-test/fulltext")
        .header("content-type", "multipart/form-data; boundary=boundary")
        .body(Body::from(multipart))
        .unwrap();
    let response = app.clone().oneshot(upload_request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let uri = "/api/v1/bib/articles/doi%3A10.1000%2Freextract-test/fulltext";
    let body = wait_for_extraction(&app, uri).await;
    assert_eq!(body["fulltext"]["extract_status"], "done");

    // Re-extract resets the row and finishes again with the same text.
    let response = app
        .clone()
        .oneshot(request("POST", &format!("{uri}/reextract"), None))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: serde_json::Value =
        serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap();
    // The row may already be claimed (running) by the spawned task.
    let status = body["fulltext"]["extract_status"].as_str().unwrap();
    assert!(
        status == "pending" || status == "running",
        "status was {status}"
    );

    let body = wait_for_extraction(&app, uri).await;
    assert_eq!(body["fulltext"]["text_content"], "re-extract these bytes");
}

#[tokio::test]
async fn bib_serves_and_cleans_extracted_figures() {
    let (app, shared, _directory) = build_app_with_vfs().await;

    let response = app
        .clone()
        .oneshot(request(
            "POST",
            "/api/v1/bib/articles",
            Some(r#"{ "title": "Figure test", "doi": "10.1000/figure-test" }"#.to_owned()),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);

    let content = b"figure test source";
    let multipart = format!(
        "--boundary\r\ncontent-disposition: form-data; name=\"file\"; filename=\"note.txt\"\r\ncontent-type: text/plain\r\n\r\n{}\r\n--boundary--\r\n",
        String::from_utf8_lossy(content)
    );
    let upload_request = Request::builder()
        .method("POST")
        .uri("/api/v1/bib/articles/doi%3A10.1000%2Ffigure-test/fulltext")
        .header("content-type", "multipart/form-data; boundary=boundary")
        .body(Body::from(multipart))
        .unwrap();
    let response = app.clone().oneshot(upload_request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let uri = "/api/v1/bib/articles/doi%3A10.1000%2Ffigure-test/fulltext";
    wait_for_extraction(&app, uri).await;

    // Simulate a MinerU run: rewrite the row as markdown referencing two
    // figures and store their bytes exactly as extraction would.
    let article_id = "doi:10.1000/figure-test";
    let attachment_key = shared
        .bib
        .get_fulltext(article_id)
        .await
        .unwrap()
        .unwrap()
        .attachment_key
        .unwrap();
    shared
        .bib
        .record_extraction_success(
            article_id,
            "![](images/aaa.jpg) text ![](images/bbb.png)",
            bib_types::TextFormat::Markdown,
            "test-markdown",
        )
        .await
        .unwrap();
    let storage = shared.file_storage.as_ref().unwrap();
    let aaa = bib_base::vfs_virtual_path(&bib_base::attachment_figure_object_path(
        article_id,
        &attachment_key,
        "aaa.jpg",
    ))
    .unwrap();
    storage.write_bytes(&aaa, vec![1, 2, 3]).await.unwrap();
    let bbb = bib_base::vfs_virtual_path(&bib_base::attachment_figure_object_path(
        article_id,
        &attachment_key,
        "bbb.png",
    ))
    .unwrap();
    storage.write_bytes(&bbb, vec![4]).await.unwrap();

    let base = "/api/v1/bib/articles/doi%3A10.1000%2Ffigure-test/fulltext/images";
    let response = app
        .clone()
        .oneshot(request("GET", &format!("{base}/aaa.jpg"), None))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers().get("content-type").unwrap(),
        "image/jpeg"
    );
    assert_eq!(response.headers().get("content-length").unwrap(), "3");
    assert!(
        response
            .headers()
            .get("content-disposition")
            .unwrap()
            .to_str()
            .unwrap()
            .starts_with("inline;")
    );
    let body = response.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(&body[..], &[1, 2, 3]);

    let response = app
        .clone()
        .oneshot(request("GET", &format!("{base}/bbb.png"), None))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers().get("content-type").unwrap(), "image/png");
    let _ = response.into_body().collect().await.unwrap().to_bytes();

    // Missing figure, missing article, unsafe or unsupported names.
    let response = app
        .clone()
        .oneshot(request("GET", &format!("{base}/missing.jpg"), None))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let response = app
        .clone()
        .oneshot(request(
            "GET",
            "/api/v1/bib/articles/doi%3A10.1000%2Funknown-article/fulltext/images/aaa.jpg",
            None,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let response = app
        .clone()
        .oneshot(request("GET", &format!("{base}/.."), None))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let response = app
        .clone()
        .oneshot(request("GET", &format!("{base}/.hidden.jpg"), None))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let response = app
        .clone()
        .oneshot(request("GET", &format!("{base}/notes.txt"), None))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    // Re-extraction clears the markdown row, which takes its figures along.
    let response = app
        .clone()
        .oneshot(request("POST", &format!("{uri}/reextract"), None))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    wait_for_extraction(&app, uri).await;
    for name in ["aaa.jpg", "bbb.png"] {
        let response = app
            .clone()
            .oneshot(request("GET", &format!("{base}/{name}"), None))
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            StatusCode::NOT_FOUND,
            "{name} should be gone after re-extraction"
        );
    }
    assert!(storage.content_length(&aaa).await.is_err());
}

#[tokio::test]
async fn bib_reextract_rejects_non_vfs_sources() {
    let shared = BibShared::open_in_memory().await.unwrap();
    let article = bib_types::Article::new("doi:10.1000/inline-source", "Inline source");
    shared.bib.upsert_article(&article).await.unwrap();
    shared
        .bib
        .upsert_fulltext(&bib_types::FullText {
            article_id: article.id.clone(),
            attachment_key: None,
            file_path: "europepmc:PMC123456/fullTextXML".to_owned(),
            file_format: bib_types::FileFormat::Html,
            text_content: Some("<p>inline text</p>".to_owned()),
            source: bib_types::FullTextSource::OpenAccess,
            file_hash: None,
            file_size: None,
            uploaded_at: None,
            extract_status: Some(bib_types::ExtractStatus::Done),
            text_format: Some(bib_types::TextFormat::Plain),
            extracted_by: Some("europepmc".to_owned()),
            extract_error: None,
        })
        .await
        .unwrap();
    let app = api_server::api_router(shared.clone(), wiki_handle().await);

    let response = app
        .clone()
        .oneshot(request(
            "POST",
            "/api/v1/bib/articles/doi%3A10.1000%2Finline-source/fulltext/reextract",
            None,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body: serde_json::Value =
        serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert!(
        body["error"].as_str().unwrap().contains("re-upload"),
        "error was: {body}"
    );
}

#[tokio::test]
async fn bib_upload_stores_original_pdf_bytes_with_application_pdf_mime() {
    let (app, _shared, _directory) = build_app_with_vfs().await;

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
    let app = api_server::api_router_with_auth(
        shared,
        wiki_handle().await,
        Some("test-token".to_owned()),
    );

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
}

#[tokio::test]
async fn semantic_configuration_is_explicit_validated_and_authenticated() {
    let shared = BibShared::open_in_memory().await.unwrap();
    let app = api_server::api_router_with_auth(
        shared,
        wiki_handle().await,
        Some("semantic-test-token".into()),
    );
    let path = "/api/v1/bib/search/semantic";
    let discovery = app
        .clone()
        .oneshot(request(
            "POST",
            "/api/v1/bib/search/semantic/models",
            Some("{}".into()),
        ))
        .await
        .unwrap();
    assert_eq!(discovery.status(), StatusCode::UNAUTHORIZED);
    let maintenance = app
        .clone()
        .oneshot(request(
            "POST",
            "/api/v1/bib/search/semantic/maintenance",
            Some("{}".into()),
        ))
        .await
        .unwrap();
    assert_eq!(maintenance.status(), StatusCode::UNAUTHORIZED);
    for method in ["GET", "PUT"] {
        let response = app
            .clone()
            .oneshot(request(
                method,
                path,
                if method == "PUT" {
                    Some("{}".into())
                } else {
                    None
                },
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }
    let authenticated = |method: &str, body: Option<String>| {
        let mut request = request(method, path, body);
        request.headers_mut().insert(
            "authorization",
            "Bearer semantic-test-token".parse().unwrap(),
        );
        request
    };
    let response = app
        .clone()
        .oneshot(authenticated("GET", None))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let status: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(status["phase"], "disabled");
    assert_eq!(status["config"]["enabled"], false);
    let before = status["configuration_generation"].clone();
    for invalid in [
        serde_json::json!({"enabled":true}),
        serde_json::json!({"origin":"https://embedding.example"}),
        serde_json::json!({"unknown":true}),
        serde_json::json!({"inference":{"num_ctx":0}}),
        serde_json::json!({"inference":{"num_batch":8193}}),
        serde_json::json!({"inference":{"unknown":1}}),
        serde_json::json!({"limits":{"max_chunks":0}}),
        serde_json::json!({"limits":{"max_vector_bytes":0}}),
        serde_json::json!({"limits":{"max_disk_bytes":0}}),
        serde_json::json!({"limits":{"unknown":1}}),
    ] {
        let response = app
            .clone()
            .oneshot(authenticated("PUT", Some(invalid.to_string())))
            .await
            .unwrap();
        assert!(response.status().is_client_error());
    }
    let response = app
        .clone()
        .oneshot(authenticated("GET", None))
        .await
        .unwrap();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let status: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(status["configuration_generation"], before);
    let response = app
        .clone()
        .oneshot(authenticated("PUT", Some("{}".into())))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let accepted: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_ne!(accepted["configuration_generation"], before);
    assert_eq!(accepted["status"]["phase"], "disabled");
    let response = app
        .clone()
        .oneshot(authenticated(
            "PUT",
            Some(
                serde_json::json!({"limits":{"max_chunks":25,"max_vector_bytes":1048576}})
                    .to_string(),
            ),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let accepted: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(accepted["status"]["config"]["limits"]["max_chunks"], 25);
    assert_eq!(
        accepted["status"]["config"]["limits"]["max_vector_bytes"],
        1048576
    );
    assert_eq!(
        accepted["status"]["config"]["limits"]["max_disk_bytes"],
        8u64 * 1024 * 1024 * 1024
    );
}

#[tokio::test]
async fn hybrid_http_uses_pinned_provider_hard_filters_evidence_and_bounded_export() {
    // A real local HTTP model protocol fixture, not a relevance evaluation.
    let digest = "a".repeat(64);
    let tags = serde_json::json!({"models":[{"name":format!("autonomics-embedding:{digest}"),"digest":digest},{"name":"fixture:latest","digest":digest}]});
    let model = axum::Router::new()
        .route(
            "/api/tags",
            axum::routing::get(move || {
                let tags = tags.clone();
                async move { axum::Json(tags) }
            }),
        )
        .route(
            "/api/embed",
            axum::routing::post(
                |axum::Json(body): axum::Json<serde_json::Value>| async move {
                    assert_eq!(body["truncate"], false);
                    assert_eq!(
                        body["options"],
                        serde_json::json!({"num_ctx":8192,"num_batch":8192})
                    );
                    assert!(
                        body["model"]
                            .as_str()
                            .unwrap()
                            .starts_with("autonomics-embedding:")
                    );
                    let inputs = body["input"].as_array().unwrap();
                    axum::Json(serde_json::json!({"embeddings":vec![vec![1.,0.];inputs.len()]}))
                },
            ),
        );
    let server = api_server::start(model, "127.0.0.1:0").await.unwrap();
    let shared = BibShared::open_in_memory().await.unwrap();
    for (id, year) in [("a", 2026), ("b", 2010)] {
        let mut article = bib_types::Article::new(id, format!("Scientific paper {id}"));
        article.year = Some(year);
        article.abstract_text = Some(format!("原始摘要 {id}，无需逐字命中主题。"));
        shared.bib.upsert_article(&article).await.unwrap();
    }
    let app = api_server::api_router(shared.clone(), wiki_handle().await);
    let search = serde_json::json!({"mode":"hybrid","semantic_query":"xylophone","query":"year:2026","include_articles":true});
    let response = app
        .clone()
        .oneshot(request(
            "POST",
            "/api/v1/bib/search/semantic/models",
            Some(serde_json::json!({"origin":format!("http://{}",server.addr())}).to_string()),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let installed: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(installed["models"].as_array().unwrap().len(), 2);
    assert!(
        installed["models"]
            .as_array()
            .unwrap()
            .iter()
            .any(|model| model["name"] == "fixture:latest" && model["digest"] == digest)
    );
    let response = app
        .clone()
        .oneshot(request(
            "POST",
            "/api/v1/bib/search/semantic/models",
            Some(serde_json::json!({"origin":"https://embedding.example"}).to_string()),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let unavailable = app
        .clone()
        .oneshot(request(
            "POST",
            "/api/v1/bib/search/v2",
            Some(search.to_string()),
        ))
        .await
        .unwrap();
    assert_eq!(unavailable.status(), StatusCode::SERVICE_UNAVAILABLE);
    for invalid in [
        serde_json::json!({"mode":"hybrid"}),
        serde_json::json!({"semantic_query":"topic"}),
        serde_json::json!({"mode":"hybrid","semantic_query":"   "}),
    ] {
        let response = app
            .clone()
            .oneshot(request(
                "POST",
                "/api/v1/bib/search/v2",
                Some(invalid.to_string()),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }
    let config = serde_json::json!({"enabled":true,"origin":format!("http://{}",server.addr()),"model":"fixture:latest","model_digest":digest,"profile":{"dimensions":2},"inference":{"num_ctx":8192,"num_batch":8192}});
    let response = app
        .clone()
        .oneshot(request(
            "PUT",
            "/api/v1/bib/search/semantic",
            Some(config.to_string()),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    tokio::time::timeout(std::time::Duration::from_secs(8), async {
        loop {
            let response = app
                .clone()
                .oneshot(request("GET", "/api/v1/bib/search/semantic", None))
                .await
                .unwrap();
            let bytes = response.into_body().collect().await.unwrap().to_bytes();
            let status: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            assert_ne!(status["phase"], "failed", "{status}");
            if status["phase"] == "ready" {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    for action in ["rebuild", "repair", "cancel", "resume"] {
        let response = app
            .clone()
            .oneshot(request("GET", "/api/v1/bib/search/semantic", None))
            .await
            .unwrap();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        let before: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        let id = uuid::Uuid::new_v4().to_string();
        let command = serde_json::json!({"action":action,"expected_configuration_generation":before["configuration_generation"],"request_id":id});
        for _ in 0..2 {
            let response = app
                .clone()
                .oneshot(request(
                    "POST",
                    "/api/v1/bib/search/semantic/maintenance",
                    Some(command.to_string()),
                ))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::ACCEPTED);
            let bytes = response.into_body().collect().await.unwrap().to_bytes();
            let ack: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(ack["maintenance"]["request_id"], id);
        }
        tokio::time::timeout(std::time::Duration::from_secs(8), async {
            loop {
                let response = app
                    .clone()
                    .oneshot(request("GET", "/api/v1/bib/search/semantic", None))
                    .await
                    .unwrap();
                let bytes = response.into_body().collect().await.unwrap().to_bytes();
                let status: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
                assert_ne!(status["phase"], "failed", "{status}");
                if status["maintenance"]["request_id"] == id
                    && (if action == "cancel" {
                        status["phase"] == "paused"
                    } else {
                        status["maintenance"]["state"] == "complete"
                    })
                {
                    if action == "repair" {
                        assert!(!status["store_generation"].as_str().unwrap().is_empty());
                    }
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap();
    }
    let response = app
        .clone()
        .oneshot(request(
            "POST",
            "/api/v1/bib/search/v2",
            Some(search.to_string()),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let page: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(page["total"], 1);
    assert_eq!(page["hits"][0]["article_id"], "a");
    assert_eq!(page["hits"][0]["snippet"], "原始摘要 a，无需逐字命中主题。");
    assert_eq!(page["hits"][0]["highlight_ranges"], serde_json::json!([]));
    assert_eq!(page["retrieval"]["total_kind"], "candidate_pool");
    assert_eq!(page["retrieval"]["eligible_articles"], 1);
    assert_eq!(
        page["retrieval"]["hits"]["a"]["lexical_rank"],
        serde_json::Value::Null
    );
    let response = app
        .clone()
        .oneshot(request(
            "POST",
            "/api/v1/bib/articles/a/search-evidence",
            Some(
                serde_json::json!({"search":search,"index_generation":page["index_generation"]})
                    .to_string(),
            ),
        ))
        .await
        .unwrap();
    // Evidence path shares the same generation and source object checks.
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let evidence: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(
        evidence["evidence"][0]["snippet"],
        page["hits"][0]["snippet"]
    );
    assert!(evidence["evidence"][0]["semantic_evidence"].is_object());
    let response = app
        .clone()
        .oneshot(request(
            "POST",
            "/api/v1/bib/search/export",
            Some(serde_json::json!({"search":search,"format":"ris"}).to_string()),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let exported: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(exported["count"], 1);
    assert_eq!(exported["result_basis"], "candidate_pool");
    assert!(
        exported["export"]
            .as_str()
            .unwrap()
            .contains("Scientific paper a")
    );
    let mut limited = config.clone();
    limited["limits"] = serde_json::json!({"max_input_bytes":64});
    let response = app
        .clone()
        .oneshot(request(
            "PUT",
            "/api/v1/bib/search/semantic",
            Some(limited.to_string()),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let epoch: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(8), async {
        loop {
            let response = app
                .clone()
                .oneshot(request("GET", "/api/v1/bib/search/semantic", None))
                .await
                .unwrap();
            let bytes = response.into_body().collect().await.unwrap().to_bytes();
            let status: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            if status["configuration_generation"] == epoch["configuration_generation"]
                && status["phase"] == "ready"
            {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    let response = app
        .clone()
        .oneshot(request(
            "POST",
            "/api/v1/bib/search/v2",
            Some(
                serde_json::json!({"mode":"hybrid","semantic_query":"immune gene expression"})
                    .to_string(),
            ),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let failure: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert!(
        failure["error"]
            .as_str()
            .unwrap()
            .contains("model query input bytes")
    );
    let response = app
        .clone()
        .oneshot(request(
            "PUT",
            "/api/v1/bib/search/semantic",
            Some("{}".into()),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    let response = app
        .oneshot(request(
            "POST",
            "/api/v1/bib/search/v2",
            Some(search.to_string()),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    server.shutdown().await.unwrap();
}

#[test]
fn http_api_defaults_to_localhost() {
    assert_eq!(api_server::DEFAULT_HTTP_API_ADDR, "127.0.0.1:8765");
}

/// Poll a full-text row until the background extraction reaches a
/// terminal state. Panics on `failed` or after ~5 s without progress.
async fn wait_for_extraction(app: &axum::Router, uri: &str) -> serde_json::Value {
    for _ in 0..250 {
        let response = app
            .clone()
            .oneshot(request("GET", uri, None))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
        match body["fulltext"]["extract_status"].as_str() {
            Some("done") => return body,
            Some("failed") => panic!(
                "background extraction failed: {:?}",
                body["fulltext"]["extract_error"]
            ),
            _ => {}
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    panic!("background extraction did not finish within 5 s ({uri})");
}

#[tokio::test]
async fn bib_supports_multiple_zotero_attachments() {
    let (app, _shared, _directory) = build_app_with_vfs().await;

    let response = app
        .clone()
        .oneshot(request(
            "POST",
            "/api/v1/bib/articles",
            Some(
                r#"{ "title": "Multi attachment", "doi": "10.1000/multi-attachment" }"#.to_owned(),
            ),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);

    let upload = |body: String| {
        Request::builder()
            .method("POST")
            .uri("/api/v1/bib/articles/doi%3A10.1000%2Fmulti-attachment/attachments")
            .header("content-type", "multipart/form-data; boundary=boundary")
            .body(Body::from(body))
            .unwrap()
    };
    let first = upload(
        "--boundary\r\ncontent-disposition: form-data; name=\"file\"; filename=\"first.txt\"\r\ncontent-type: text/plain\r\n\r\nfirst attachment\r\n--boundary--\r\n".to_owned(),
    );
    let response = app.clone().oneshot(first).await.unwrap();
    let status = response.status();
    let response_bytes = response.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(
        status,
        StatusCode::CREATED,
        "{}",
        String::from_utf8_lossy(&response_bytes)
    );
    let body: serde_json::Value = serde_json::from_slice(&response_bytes).unwrap();
    let first_key = body["attachment"]["key"].as_str().unwrap().to_owned();
    assert!(
        body["attachment"]["vfs_path"]
            .as_str()
            .unwrap()
            .contains("/attachments/")
    );

    let second = upload(
        "--boundary\r\ncontent-disposition: form-data; name=\"file\"; filename=\"second.txt\"\r\ncontent-type: text/plain\r\n\r\nsecond attachment\r\n--boundary--\r\n".to_owned(),
    );
    let response = app.clone().oneshot(second).await.unwrap();
    let status = response.status();
    let response_bytes = response.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(
        status,
        StatusCode::CREATED,
        "{}",
        String::from_utf8_lossy(&response_bytes)
    );
    let body: serde_json::Value = serde_json::from_slice(&response_bytes).unwrap();
    let second_key = body["attachment"]["key"].as_str().unwrap().to_owned();
    assert_ne!(first_key, second_key);

    let first_fulltext_uri = format!(
        "/api/v1/bib/articles/doi%3A10.1000%2Fmulti-attachment/attachments/{first_key}/fulltext"
    );
    let first_fulltext = wait_for_extraction(&app, &first_fulltext_uri).await;
    assert_eq!(
        first_fulltext["fulltext"]["text_content"],
        "first attachment"
    );
    assert_eq!(first_fulltext["fulltext"]["attachment_key"], first_key);

    let response = app
        .clone()
        .oneshot(request(
            "GET",
            "/api/v1/bib/articles/doi%3A10.1000%2Fmulti-attachment/attachments",
            None,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: serde_json::Value =
        serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(body["attachments"].as_array().unwrap().len(), 2);
    let hash = body["attachments"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["key"] == second_key)
        .unwrap()["storage_hash"]
        .as_str()
        .unwrap();
    for (expected, status) in [
        (hash, StatusCode::OK),
        ("outdated-hash", StatusCode::CONFLICT),
    ] {
        let response=app.clone().oneshot(request("GET",&format!("/api/v1/bib/articles/doi%3A10.1000%2Fmulti-attachment/attachments/{second_key}/raw?expected_hash={expected}"),None)).await.unwrap();
        assert_eq!(response.status(), status);
    }

    let response = app
        .clone()
        .oneshot(request(
            "GET",
            &format!(
                "/api/v1/bib/articles/doi%3A10.1000%2Fmulti-attachment/attachments/{second_key}/raw"
            ),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let raw = response.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(&raw[..], b"second attachment");

    let annotation_uri = format!(
        "/api/v1/bib/articles/doi%3A10.1000%2Fmulti-attachment/attachments/{second_key}/annotations"
    );
    let response = app
        .clone()
        .oneshot(request(
            "POST",
            &annotation_uri,
            Some(
                r##"{ "kind": "highlight", "content": "nested highlight", "page": 2, "color": "#ffd400" }"##
                    .to_owned(),
            ),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
    let body: serde_json::Value =
        serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap();
    let annotation_id = body["annotation"]["id"].as_str().unwrap().to_owned();
    assert_eq!(body["annotation"]["attachment_key"], second_key);
    assert!(body["annotation"]["item_key"].as_str().is_some());

    let response = app
        .clone()
        .oneshot(request("GET", &annotation_uri, None))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: serde_json::Value =
        serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(body["annotations"].as_array().unwrap().len(), 1);
    assert_eq!(body["annotations"][0]["attachment_key"], second_key);

    let response = app
        .clone()
        .oneshot(request(
            "GET",
            "/api/v1/bib/articles?query=multi%20attachment&limit=10",
            None,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: serde_json::Value =
        serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap();
    let summary = &body["attachment_summaries"]["doi:10.1000/multi-attachment"];
    assert_eq!(summary["file"], true);
    assert_eq!(summary["note"], true);
    assert_eq!(summary["count"], 3);
    assert_eq!(summary["pdf"], false);

    let response = app
        .clone()
        .oneshot(request(
            "DELETE",
            &format!("/api/v1/bib/annotations/{annotation_id}"),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let response = app
        .clone()
        .oneshot(request(
            "PUT",
            &format!(
                "/api/v1/bib/articles/doi%3A10.1000%2Fmulti-attachment/attachments/{second_key}/preferred"
            ),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    wait_for_extraction(
        &app,
        "/api/v1/bib/articles/doi%3A10.1000%2Fmulti-attachment/fulltext",
    )
    .await;
    let body = wait_for_extraction(
        &app,
        "/api/v1/bib/articles/doi%3A10.1000%2Fmulti-attachment/fulltext",
    )
    .await;
    assert_eq!(body["fulltext"]["text_content"], "second attachment");
    assert!(
        body["fulltext"]["file_path"]
            .as_str()
            .unwrap()
            .contains(&format!("/attachments/{second_key}/"))
    );

    let response = app
        .clone()
        .oneshot(request(
            "GET",
            "/api/v1/bib/articles?query=first%20attachment&limit=10",
            None,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: serde_json::Value =
        serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(body["total"], 1);

    let second_fulltext_uri = format!(
        "/api/v1/bib/articles/doi%3A10.1000%2Fmulti-attachment/attachments/{second_key}/fulltext/reextract"
    );
    let response = app
        .clone()
        .oneshot(request("POST", &second_fulltext_uri, None))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let second_fulltext_uri = second_fulltext_uri
        .trim_end_matches("/reextract")
        .to_owned();
    let second_fulltext = wait_for_extraction(&app, &second_fulltext_uri).await;
    assert_eq!(
        second_fulltext["fulltext"]["text_content"],
        "second attachment"
    );

    let response = app
        .clone()
        .oneshot(request(
            "DELETE",
            &format!(
                "/api/v1/bib/articles/doi%3A10.1000%2Fmulti-attachment/attachments/{second_key}"
            ),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let response = app
        .oneshot(request(
            "GET",
            "/api/v1/bib/articles/doi%3A10.1000%2Fmulti-attachment/attachments",
            None,
        ))
        .await
        .unwrap();
    let body: serde_json::Value =
        serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(body["attachments"].as_array().unwrap().len(), 1);
    assert_eq!(body["attachments"][0]["key"], first_key);
}

async fn build_app_with_vfs() -> (axum::Router, BibShared, tempfile::TempDir) {
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
            permissions: Default::default(),
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
    let app = api_server::api_router(shared.clone(), wiki_handle().await);
    (app, shared, directory)
}

#[tokio::test]
async fn bib_structured_fulltext_cache_is_attachment_scoped_and_invalidated_on_reextract() {
    use bib_base::zotero_extract::store_structure;
    use bib_types::{Article, FileFormat, FullText, FullTextSource};
    let (app, shared, _directory) = build_app_with_vfs().await;
    let id = "doi:10.1000/reading-mode";
    shared
        .bib
        .upsert_article(&Article::new(id, "Semantic PDF"))
        .await
        .unwrap();
    let stored = bib_base::stored_fulltext(id, "paper.pdf", b"source PDF");
    let storage = shared.file_storage.as_ref().unwrap();
    storage
        .write_bytes(
            &bib_base::vfs_virtual_path(&stored.path).unwrap(),
            b"source PDF".to_vec(),
        )
        .await
        .unwrap();
    shared
        .bib
        .upsert_fulltext(&FullText {
            article_id: id.into(),
            attachment_key: None,
            file_path: stored.path,
            file_format: FileFormat::Pdf,
            text_content: Some("old continuous text".into()),
            source: FullTextSource::UserUpload,
            file_hash: Some(stored.file_hash),
            file_size: Some(10),
            uploaded_at: None,
            extract_status: Some(bib_types::ExtractStatus::Done),
            text_format: Some(bib_types::TextFormat::Plain),
            extracted_by: Some("simple".into()),
            extract_error: None,
        })
        .await
        .unwrap();
    let base = "/api/v1/bib/articles/doi%3A10.1000%2Freading-mode";
    let endpoint = format!("{base}/fulltext/structured");
    let response = app
        .clone()
        .oneshot(request("GET", &endpoint, None))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let attachment = shared.bib.zotero_attachment(id).await.unwrap().unwrap();
    let fulltext = shared.bib.get_fulltext(id).await.unwrap().unwrap();
    let structure = serde_json::json!({
        "schemaVersion": "1.2.0", "metadata": { "processor": { "type": "pdf", "version": 15 } },
        "catalog": { "pages": [], "outline": [] },
        "content": [{ "type": "paragraph", "content": [{ "text": "First paragraph." }] }]
    });
    store_structure(
        storage,
        id,
        Some(&attachment.key),
        &fulltext,
        Some(&structure),
    )
    .await
    .unwrap();
    for method in ["GET", "POST"] {
        for url in [
            endpoint.clone(),
            format!("{base}/attachments/{}/fulltext/structured", attachment.key),
        ] {
            let response = app
                .clone()
                .oneshot(request(method, &url, None))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            let body: serde_json::Value =
                serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes())
                    .unwrap();
            assert_eq!(body["structure"], structure);
        }
    }
    // A processor upgrade or source replacement must not reuse old coordinates.
    let mut stale = structure.clone();
    stale["metadata"]["processor"]["version"] = 14.into();
    store_structure(storage, id, Some(&attachment.key), &fulltext, Some(&stale))
        .await
        .unwrap();
    let response = app
        .clone()
        .oneshot(request("GET", &endpoint, None))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    store_structure(
        storage,
        id,
        Some(&attachment.key),
        &fulltext,
        Some(&structure),
    )
    .await
    .unwrap();
    let response = app
        .clone()
        .oneshot(request("DELETE", &format!("{base}/fulltext"), None))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let path = bib_base::vfs_virtual_path(&bib_base::stored_files::structured_object_path(
        id,
        Some(&attachment.key),
    ))
    .unwrap();
    assert!(
        storage.content_length(&path).await.is_err(),
        "deleting an attachment must remove its source map"
    );
    let response = app.oneshot(request("GET", &endpoint, None)).await.unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
#[ignore = "requires Node >=22 and npm ci in tools/zotero-extractor"]
async fn bib_zotero_migrates_flattened_text_on_reading_mode_open() {
    use bib_types::{Article, FileFormat, FullText, FullTextSource};
    let (app, shared, _directory) = build_app_with_vfs().await;
    let id = "doi:10.1000/zotero-migration";
    let bytes =
        include_bytes!("../../../tools/zotero-extractor/test/fixtures/synthetic-list-links.pdf");
    shared
        .bib
        .upsert_article(&Article::new(id, "Semantic migration"))
        .await
        .unwrap();
    let stored = bib_base::stored_fulltext(id, "paper.pdf", bytes);
    let storage = shared.file_storage.as_ref().unwrap();
    storage
        .write_bytes(
            &bib_base::vfs_virtual_path(&stored.path).unwrap(),
            bytes.to_vec(),
        )
        .await
        .unwrap();
    shared
        .bib
        .upsert_fulltext(&FullText {
            article_id: id.into(),
            attachment_key: None,
            file_path: stored.path,
            file_format: FileFormat::Pdf,
            text_content: Some("old continuous text".into()),
            source: FullTextSource::UserUpload,
            file_hash: Some(stored.file_hash),
            file_size: Some(bytes.len() as i64),
            uploaded_at: None,
            extract_status: Some(bib_types::ExtractStatus::Done),
            text_format: Some(bib_types::TextFormat::Plain),
            extracted_by: Some("simple".into()),
            extract_error: None,
        })
        .await
        .unwrap();
    let url = "/api/v1/bib/articles/doi%3A10.1000%2Fzotero-migration/fulltext/structured";
    let response = app
        .clone()
        .oneshot(request("POST", url, None))
        .await
        .unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(
        status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&bytes)
    );
    let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["structure"]["content"][1]["type"], "paragraph");
    let fulltext = shared.bib.get_fulltext(id).await.unwrap().unwrap();
    assert_eq!(fulltext.extracted_by.as_deref(), Some("zotero-sdt"));
    assert!(
        fulltext
            .text_content
            .as_deref()
            .unwrap()
            .contains("Synthetic reference links\n\nThe invented sources")
    );
    let key = shared.bib.zotero_attachment(id).await.unwrap().unwrap().key;
    let path = bib_base::vfs_virtual_path(&bib_base::extracted_attachment_object_path(
        id,
        &key,
        bib_types::TextFormat::Plain,
    ))
    .unwrap();
    let text = bib_base::read_full(storage, &path).await.unwrap();
    assert_eq!(
        String::from_utf8(text).unwrap(),
        fulltext.text_content.unwrap()
    );
    let response = app.oneshot(request("GET", url, None)).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn bib_annotation_metadata_updates_preserve_quote_geometry_and_survive_reopen() {
    let directory = tempfile::tempdir().unwrap();
    let database = directory.path().join("annotations.db");
    let shared = BibShared::open(database.to_str().unwrap()).await.unwrap();
    shared
        .bib
        .upsert_article(&bib_types::Article::new(
            "annotation-edit",
            "Annotation editing",
        ))
        .await
        .unwrap();
    let rects = vec![bib_types::AnnotationRect {
        left: 0.1,
        top: 0.2,
        width: 0.4,
        height: 0.03,
        space: Some("pdf".into()),
        text_range: Some(bib_types::AnnotationTextRange {
            source_hash: "source-pdf-hash".into(),
            start: vec![1, 0, 8],
            end: vec![1, 2, 17],
            quote: "Unchanged source quote".into(),
        }),
    }];
    let original = shared
        .bib
        .add_annotation(
            "annotation-edit",
            bib_types::AnnotationKind::Highlight,
            "Unchanged source quote",
            Some(5),
            Some("yellow".into()),
            Some(rects.clone()),
            None,
        )
        .await
        .unwrap();
    let app = api_server::api_router(shared.clone(), wiki_handle().await);
    let url = format!("/api/v1/bib/annotations/{}", original.id);
    let response = app.clone().oneshot(request("PATCH", &url, Some(serde_json::json!({ "comment": "A persistent interpretation", "tags": ["brain", "methods", "brain"], "color": "#ff6666" }).to_string()))).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(
        status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&bytes)
    );
    let result: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(result["annotation"]["content"], "Unchanged source quote");
    assert_eq!(
        result["annotation"]["comment"],
        "A persistent interpretation"
    );
    assert_eq!(
        result["annotation"]["tags"],
        serde_json::json!(["brain", "methods"])
    );
    assert_eq!(result["annotation"]["page"], 5);
    assert_eq!(
        result["annotation"]["rects"],
        serde_json::to_value(&rects).unwrap()
    );
    let reopened = BibShared::open(database.to_str().unwrap()).await.unwrap();
    let saved = reopened
        .bib
        .list_annotations("annotation-edit", None)
        .await
        .unwrap();
    assert_eq!(saved[0].comment, "A persistent interpretation");
    assert_eq!(saved[0].tags, vec!["brain", "methods"]);
    assert_eq!(saved[0].rects, Some(rects));
    for body in [
        serde_json::json!({}),
        serde_json::json!({"color": "url(bad)"}),
        serde_json::json!({"tags": [""]}),
    ] {
        let response = app
            .clone()
            .oneshot(request("PATCH", &url, Some(body.to_string())))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }
    let response = app
        .clone()
        .oneshot(request(
            "PATCH",
            "/api/v1/bib/annotations/missing",
            Some("{\"comment\":\"x\"}".into()),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let response = app
        .oneshot(request(
            "PATCH",
            &url,
            Some("{\"comment\":\"\",\"tags\":[]}".into()),
        ))
        .await
        .unwrap();
    let status = response.status();
    let body = response.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    let cleared = shared
        .bib
        .list_annotations("annotation-edit", None)
        .await
        .unwrap();
    assert_eq!(cleared[0].comment, "");
    assert!(cleared[0].tags.is_empty());
    assert_eq!(cleared[0].content, original.content);
}

#[tokio::test]
async fn nested_collections_api_moves_trashes_restores_and_preserves_provenance() {
    let shared = BibShared::open_in_memory().await.unwrap();
    let db = shared.bib.clone();
    let app = api_server::api_router(shared.clone(), wiki_handle().await);
    async fn send(
        app: &axum::Router,
        method: &str,
        path: &str,
        value: serde_json::Value,
    ) -> (StatusCode, serde_json::Value) {
        let response = app
            .clone()
            .oneshot(request(
                method,
                path,
                if value.is_null() {
                    None
                } else {
                    Some(value.to_string())
                },
            ))
            .await
            .unwrap();
        let status = response.status();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        (status, serde_json::from_slice(&bytes).unwrap())
    }
    let (status, a) = send(
        &app,
        "POST",
        "/api/v1/bib/collections",
        serde_json::json!({"name":"Parent"}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let aid = a["collection"]["id"].as_str().unwrap();
    assert!(a["collection"]["collection_id"].as_i64().unwrap() > 0);
    assert!(!a["collection"]["key"].as_str().unwrap().is_empty());
    let (_, b) = send(
        &app,
        "POST",
        "/api/v1/bib/collections",
        serde_json::json!({"name":"Child","parent_id":aid}),
    )
    .await;
    let bid = b["collection"]["id"].as_str().unwrap();
    db.upsert_article(&bib_base::Article::new("p", "Shared"))
        .await
        .unwrap();
    for id in [aid, bid] {
        send(
            &app,
            "POST",
            &format!("/api/v1/bib/collections/{id}/articles"),
            serde_json::json!({"article_id":"p"}),
        )
        .await;
    }
    let (status, page) = send(
        &app,
        "GET",
        &format!("/api/v1/bib/collections/{aid}/articles?include_descendants=true"),
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(page["total"], 1);
    assert_eq!(page["articles"].as_array().unwrap().len(), 1);
    assert_eq!(page["associations"].as_array().unwrap().len(), 2);
    let (status, _) = send(
        &app,
        "PATCH",
        &format!("/api/v1/bib/collections/{aid}"),
        serde_json::json!({"parent_id":bid}),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    let (status, _) = send(
        &app,
        "PATCH",
        &format!("/api/v1/bib/collections/{bid}"),
        serde_json::json!({"parent_id":"missing"}),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (_, renamed) = send(
        &app,
        "PATCH",
        &format!("/api/v1/bib/collections/{bid}"),
        serde_json::json!({"name":"Renamed"}),
    )
    .await;
    assert_eq!(renamed["collection"]["parent_id"], aid);
    let (_, moved) = send(
        &app,
        "PATCH",
        &format!("/api/v1/bib/collections/{bid}"),
        serde_json::json!({"parent_id":null}),
    )
    .await;
    assert!(moved["collection"]["parent_id"].is_null());
    send(
        &app,
        "PATCH",
        &format!("/api/v1/bib/collections/{bid}"),
        serde_json::json!({"parent_id":aid}),
    )
    .await;
    let (_, deleted) = send(
        &app,
        "DELETE",
        &format!("/api/v1/bib/collections/{aid}"),
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(deleted["deleted_ids"].as_array().unwrap().len(), 2);
    let (_, trash) = send(
        &app,
        "GET",
        "/api/v1/bib/collections/trash",
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(trash["collections"].as_array().unwrap().len(), 2);
    let (_, unfiled) = send(
        &app,
        "GET",
        "/api/v1/bib/articles/unfiled",
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(unfiled["total"], 1);
    let (status, _) = send(
        &app,
        "POST",
        &format!("/api/v1/bib/collections/{aid}/restore"),
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (_, summary) = send(
        &app,
        "GET",
        "/api/v1/bib/collections?view=summary",
        serde_json::Value::Null,
    )
    .await;
    assert!(summary["collections"][0].get("article_ids").is_none());
    let (status, _) = send(
        &app,
        "DELETE",
        &format!("/api/v1/bib/collections/{aid}?permanent=true"),
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    send(
        &app,
        "DELETE",
        &format!("/api/v1/bib/collections/{aid}"),
        serde_json::Value::Null,
    )
    .await;
    let (status, _) = send(
        &app,
        "DELETE",
        &format!("/api/v1/bib/collections/{aid}?permanent=true"),
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(db.article_count().await.unwrap(), 1);
}

#[tokio::test]
async fn bib_export_ids_exports_only_requested_records_in_order() {
    let shared = BibShared::open_in_memory().await.unwrap();
    let app = api_server::api_router(shared.clone(), wiki_handle().await);
    let records = [
        ("Alpha", "1111111"),
        ("Beta", "2222222"),
        ("Gamma", "3333333"),
    ];
    for (title, pmid) in records {
        let payload = format!(
            r#"{{ "title": "{title}", "pmid": "{pmid}", "authors": ["Smith John"], "year": 2026, "source": "manual" }}"#
        );
        let response = app
            .clone()
            .oneshot(request("POST", "/api/v1/bib/articles", Some(payload)))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);
    }

    // Explicit ids skip `limit`, ignore unknown ids and keep the given order.
    let response = app
        .oneshot(request(
            "GET",
            "/api/v1/bib/export?format=bibtex&ids=pmid:3333333,pmid:1111111",
            None,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(body["count"], 2);
    let export = body["export"].as_str().unwrap();
    let gamma = export.find("Gamma").expect("Gamma entry rendered");
    let alpha = export.find("Alpha").expect("Alpha entry rendered");
    assert!(gamma < alpha, "ids order must be preserved");
    assert!(
        !export.contains("Beta"),
        "unselected records must be excluded"
    );
}

#[tokio::test]
async fn collection_changes_wait_for_commit_and_cached_import_preserves_membership() {
    use bib_types::{AddedBy, ArticleRole, Collection, Identifier};
    let shared = BibShared::open_in_memory().await.unwrap();
    let db = shared.bib.clone();
    let app = api_server::api_router(shared.clone(), wiki_handle().await);
    let response = app
        .clone()
        .oneshot(request("GET", "/api/v1/bib/collections/changes", None))
        .await
        .unwrap();
    let body: serde_json::Value =
        serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap();
    let revision = body["revision"].as_u64().unwrap();
    let waiting = app.clone().oneshot(request(
        "GET",
        &format!("/api/v1/bib/collections/changes?since={revision}"),
        None,
    ));
    let write = async {
        db.upsert_collection(&Collection::new("collection", "Collection"))
            .await
            .unwrap();
    };
    let (_, response) = tokio::join!(write, waiting);
    let response = tokio::time::timeout(std::time::Duration::from_secs(2), async {
        response.unwrap()
    })
    .await
    .unwrap();
    let body: serde_json::Value =
        serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert!(body["revision"].as_u64().unwrap() > revision);
    let mut article = bib_base::Article::new("cached", "Cached paper");
    article
        .identifiers
        .push(Identifier::doi("10.1000/collection-cached"));
    db.upsert_article(&article).await.unwrap();
    db.add_to_collection(
        "collection",
        "cached",
        ArticleRole::Cited,
        AddedBy::Agent,
        Some("Preserve semantic note"),
    )
    .await
    .unwrap();
    let before = db
        .list_collection_articles("collection", None, None)
        .await
        .unwrap();
    for (path, body) in [
        (
            "/api/v1/bib/articles/import",
            serde_json::json!({"id_type":"doi","id":"10.1000/collection-cached","collection_id":"collection","fetch_fulltext":false}),
        ),
        (
            "/api/v1/bib/collections/collection/articles",
            serde_json::json!({"article_id":"cached"}),
        ),
    ] {
        let response = app
            .clone()
            .oneshot(request("POST", path, Some(body.to_string())))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }
    assert_eq!(
        db.list_collection_articles("collection", None, None)
            .await
            .unwrap(),
        before
    );
}

#[tokio::test]
async fn inline_pmc_text_is_repaired_for_raw_download_and_attachment_download() {
    use bib_types::{Article, ExtractStatus, FileFormat, FullText, FullTextSource, TextFormat};
    let shared = BibShared::open_in_memory()
        .await
        .unwrap()
        .with_file_storage(Arc::new(OpendalFileStorage::new_temp()));
    let db = shared.bib.clone();
    db.upsert_article(&Article::new("legacy-pmc", "PMC paper"))
        .await
        .unwrap();
    let text = "PMC text — 原文\nSecond paragraph";
    db.upsert_fulltext(&FullText {
        article_id: "legacy-pmc".into(),
        attachment_key: None,
        file_path: "europepmc:PMC42".into(),
        file_format: FileFormat::Txt,
        text_content: Some(text.into()),
        source: FullTextSource::OpenAccess,
        file_hash: None,
        file_size: Some(9999),
        uploaded_at: None,
        extract_status: Some(ExtractStatus::Done),
        text_format: Some(TextFormat::Plain),
        extracted_by: Some("europepmc".into()),
        extract_error: None,
    })
    .await
    .unwrap();
    let key = db
        .zotero_attachment("legacy-pmc")
        .await
        .unwrap()
        .unwrap()
        .key;
    let app = api_server::api_router(shared.clone(), wiki_handle().await);
    let response = app
        .clone()
        .oneshot(request(
            "GET",
            "/api/v1/bib/articles/legacy-pmc/fulltext/raw",
            None,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()["content-type"],
        "text/plain; charset=utf-8"
    );
    assert_eq!(response.headers()["content-length"], text.len().to_string());
    assert!(response.headers().contains_key("etag"));
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(&bytes[..], text.as_bytes());
    let response = app
        .clone()
        .oneshot(request(
            "GET",
            &format!("/api/v1/bib/articles/legacy-pmc/attachments/{key}/raw"),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        &response.into_body().collect().await.unwrap().to_bytes()[..],
        text.as_bytes()
    );
    let response = app
        .clone()
        .oneshot(request(
            "HEAD",
            "/api/v1/bib/articles/legacy-pmc/fulltext/raw",
            None,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["content-length"], text.len().to_string());
    assert!(
        response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes()
            .is_empty()
    );
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/bib/articles/legacy-pmc/fulltext/raw")
                .header("range", "bytes=0-2")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(
        &response.into_body().collect().await.unwrap().to_bytes()[..],
        b"PMC"
    );
}

#[tokio::test]
async fn bib_search_v2_exposes_ranked_pages_syntax_errors_and_index_progress() {
    let shared = BibShared::open_in_memory().await.unwrap();
    shared
        .bib
        .upsert_articles(&[
            bib_types::Article::new("a", "Gene editing evidence"),
            bib_types::Article::new("b", "Gene editing methods"),
        ])
        .await
        .unwrap();
    let app = api_server::api_router(shared.clone(), wiki_handle().await);
    let body = serde_json::json!({"query":"title:\"gene editing\"","page_size":1});
    let response = app
        .clone()
        .oneshot(request(
            "POST",
            "/api/v1/bib/search/v2",
            Some(body.to_string()),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let page: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(page["total"], 2);
    assert_eq!(page["hits"].as_array().unwrap().len(), 1);
    assert_eq!(page["hits"][0]["matched_field"], "title");
    assert!(page["next_cursor"].is_string());

    let next = serde_json::json!({"query":"title:\"gene editing\"","page_size":1,"cursor":page["next_cursor"]});
    let response = app
        .clone()
        .oneshot(request(
            "POST",
            "/api/v1/bib/search/v2",
            Some(next.to_string()),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let next: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_ne!(page["hits"][0]["article_id"], next["hits"][0]["article_id"]);

    let response = app
        .clone()
        .oneshot(request(
            "POST",
            "/api/v1/bib/search/v2",
            Some(r#"{"query":"title:(broken"}"#.into()),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let response = app
        .clone()
        .oneshot(request("GET", "/api/v1/bib/search/status", None))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let status: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(status["source_revision"], status["indexed_revision"]);
    assert_eq!(status["pending_articles"], 0);
    let response = app
        .oneshot(request("POST", "/api/v1/bib/search/rebuild", None))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::ACCEPTED);
}

#[tokio::test]
async fn saved_search_crud_filters_and_evidence_are_exposed_with_conflict_statuses() {
    let shared = BibShared::open_in_memory().await.unwrap();
    let mut article = bib_types::Article::new("rule-a", "Unicode 😀 Study");
    article.keywords = vec!["gene".into()];
    shared.bib.upsert_article(&article).await.unwrap();
    let app = api_server::bib::router(shared.clone());
    let response=app.clone().oneshot(request("POST","/search/v2",Some(serde_json::json!({"query":"title:Study","include_facets":true,"include_articles":true}).to_string()))).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let page: serde_json::Value =
        serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(page["articles"][0]["id"], "rule-a");
    assert_eq!(page["facets"]["tags"][0]["label"], "gene");
    let range = &page["hits"][0]["highlight_ranges"][0];
    let chars = page["hits"][0]["snippet"]
        .as_str()
        .unwrap()
        .chars()
        .collect::<Vec<_>>();
    assert_eq!(
        chars[range["start"].as_u64().unwrap() as usize..range["end"].as_u64().unwrap() as usize]
            .iter()
            .collect::<String>(),
        "Study"
    );
    for (filters, expected) in [
        (serde_json::json!({"has_annotations":true}), 0),
        (serde_json::json!({}), 1),
    ] {
        let export = serde_json::json!({"format":"bibtex","search":{"query":"title:Study","page_size":1,"filters":filters}});
        let response = app
            .clone()
            .oneshot(request("POST", "/search/export", Some(export.to_string())))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let exported: serde_json::Value =
            serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes())
                .unwrap();
        assert_eq!(exported["count"], expected);
        if expected == 1 {
            assert!(exported["export"].as_str().unwrap().contains("Study"));
        }
    }
    let body = serde_json::json!({"name":"Living review","request":{"query":"title:Study"}});
    let response = app
        .clone()
        .oneshot(request("POST", "/saved-searches", Some(body.to_string())))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
    let rule: serde_json::Value =
        serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap();
    let id = rule["saved_search"]["id"].as_str().unwrap();
    let update = serde_json::json!({"name":"Edited review","request":{"query":"title:Study"},"expected_version":1});
    let response = app
        .clone()
        .oneshot(request(
            "PUT",
            &format!("/saved-searches/{id}"),
            Some(update.to_string()),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let response = app
        .clone()
        .oneshot(request(
            "PUT",
            &format!("/saved-searches/{id}"),
            Some(update.to_string()),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);
    let response = app
        .clone()
        .oneshot(request("GET", "/saved-searches", None))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let list: serde_json::Value =
        serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(list["saved_searches"][0]["name"], "Edited review");
    let evidence = serde_json::json!({"search":{"query":"title:Study"},"index_generation":page["index_generation"]});
    let response = app
        .clone()
        .oneshot(request(
            "POST",
            "/articles/rule-a/search-evidence",
            Some(evidence.to_string()),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let data: serde_json::Value =
        serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(data["total"], 1);
    assert_eq!(data["evidence"][0]["location_status"], "metadata");
    assert!(data["evidence"][0]["page"].is_null());
    let response = app
        .clone()
        .oneshot(request(
            "DELETE",
            &format!("/saved-searches/{id}?expected_version=1"),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);
    let response = app
        .clone()
        .oneshot(request(
            "DELETE",
            &format!("/saved-searches/{id}?expected_version=2"),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    let response = app
        .oneshot(request(
            "DELETE",
            &format!("/saved-searches/{id}?expected_version=2"),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn search_request_ids_support_early_cancellation_and_safe_reuse() {
    let shared = BibShared::open_in_memory().await.unwrap();
    shared
        .bib
        .upsert_article(&bib_types::Article::new("cancel-a", "Cancellation target"))
        .await
        .unwrap();
    let app = api_server::bib::router(shared.clone());
    let response = app
        .clone()
        .oneshot(request(
            "POST",
            "/search/cancel",
            Some(serde_json::json!({"request_id":"early-search"}).to_string()),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    let mut req = request(
        "POST",
        "/search/v2",
        Some(serde_json::json!({"query":"target"}).to_string()),
    );
    req.headers_mut()
        .insert("x-search-request-id", "early-search".parse().unwrap());
    let response = app.clone().oneshot(req).await.unwrap();
    assert_eq!(response.status().as_u16(), 499);
    let mut req = request(
        "POST",
        "/search/v2",
        Some(serde_json::json!({"query":"target"}).to_string()),
    );
    req.headers_mut()
        .insert("x-search-request-id", "early-search".parse().unwrap());
    let response = app.clone().oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let response = app
        .clone()
        .oneshot(request(
            "POST",
            "/search/parse",
            Some(serde_json::json!({"query":"title:(alpha OR beta) NOT year:2020"}).to_string()),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let response = app
        .oneshot(request(
            "POST",
            "/search/cancel",
            Some(serde_json::json!({"request_id":"bad id"}).to_string()),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}
