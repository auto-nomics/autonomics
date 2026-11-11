//! Real multipart upload / raw image / cache / reference-protected deletion contract.
use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use http_body_util::BodyExt;
use std::sync::Arc;
use tower::ServiceExt;
use wiki::{
    service,
    storage::{blob::MemoryBlobStore, turso_wiki_storage::TursoWikiStorage},
};

async fn json(response: axum::response::Response) -> serde_json::Value {
    serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap()
}
fn upload(path: &str, filename: &str, bytes: &[u8]) -> Request<Body> {
    let mut body = format!("--image-boundary\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{filename}\"\r\nContent-Type: image/png\r\n\r\n").into_bytes();
    body.extend_from_slice(bytes);
    body.extend_from_slice(b"\r\n--image-boundary--\r\n");
    Request::post(path)
        .header(
            "content-type",
            "multipart/form-data; boundary=image-boundary",
        )
        .body(Body::from(body))
        .unwrap()
}
#[tokio::test]
async fn wiki_image_upload_cache_and_delete_protection() {
    let handle = Arc::new(service::start(Arc::new(
        TursoWikiStorage::open_in_memory()
            .await
            .unwrap()
            .with_blob_store(MemoryBlobStore::new()),
    )));
    let app = api_server::wiki::router(handle);
    let response = app
        .clone()
        .oneshot(
            Request::post("/entries")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"title":"图片笔记","content":""}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let entry = json(response).await;
    let id = entry["id"].as_str().unwrap();
    let path = format!("/entries/{id}/attachments");
    let bad = app
        .clone()
        .oneshot(upload(&path, "fake.png", b"not an image"))
        .await
        .unwrap();
    assert_eq!(bad.status(), StatusCode::BAD_REQUEST);
    let png = b"\x89PNG\r\n\x1a\ntest";
    let response = app
        .clone()
        .oneshot(upload(&path, "实验 [1].png", png))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let meta = json(response).await;
    let image = meta["attachment_id"].as_str().unwrap();
    let raw = format!("{path}/{image}/raw");
    let response = app
        .clone()
        .oneshot(Request::get(&raw).body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["content-type"], "image/png");
    assert_eq!(response.headers()["x-content-type-options"], "nosniff");
    assert!(
        response.headers()["content-security-policy"]
            .to_str()
            .unwrap()
            .contains("sandbox")
    );
    let etag = response.headers()["etag"].to_str().unwrap().to_owned();
    assert_eq!(
        response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes()
            .as_ref(),
        png
    );
    let cached = app
        .clone()
        .oneshot(
            Request::get(&raw)
                .header("if-none-match", format!("W/{etag}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(cached.status(), StatusCode::NOT_MODIFIED);
    assert!(
        cached
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes()
            .is_empty()
    );
    let content = serde_json::json!({ "content": wiki::attachments::markdown_ref(id, image, "实验 [1].png") });
    let put = Request::put(format!("/entries/{id}/content"))
        .header("content-type", "application/json")
        .body(Body::from(content.to_string()))
        .unwrap();
    assert_eq!(
        app.clone().oneshot(put).await.unwrap().status(),
        StatusCode::OK
    );
    let remove = app
        .oneshot(
            Request::delete(format!("{path}/{image}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(remove.status(), StatusCode::CONFLICT);
    assert!(
        json(remove).await["error"]
            .as_str()
            .unwrap()
            .contains("图片笔记")
    );
}
