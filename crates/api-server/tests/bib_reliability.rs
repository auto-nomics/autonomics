use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use bib_base::BibShared;
use bib_types::{AnnotationKind, Article};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;

async fn call(
    app: &axum::Router,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let request = Request::builder()
        .method(method)
        .uri(path)
        .header("content-type", "application/json")
        .body(
            body.map(|v| Body::from(v.to_string()))
                .unwrap_or_else(Body::empty),
        )
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}
#[tokio::test]
async fn metadata_revision_trash_restore_and_purge_are_exposed_consistently() {
    let shared = BibShared::open_in_memory().await.unwrap();
    shared
        .bib
        .upsert_article(&Article::new("paper", "Original"))
        .await
        .unwrap();
    shared
        .bib
        .add_annotation(
            "paper",
            AnnotationKind::Comment,
            "Keep this",
            None,
            None,
            None,
            None,
        )
        .await
        .unwrap();
    let app = api_server::bib::router(shared.clone());
    let (status, detail) = call(&app, "GET", "/articles/paper", None).await;
    assert_eq!(status, StatusCode::OK);
    let mut change = detail["article"].clone();
    change["title"] = json!("User edit");
    change["expected_revision"] = detail["revision"].clone();
    let (status, saved) = call(&app, "PUT", "/articles/paper", Some(change.clone())).await;
    assert_eq!(status, StatusCode::OK);
    assert_ne!(detail["revision"], saved["revision"]);
    assert_eq!(
        call(&app, "PUT", "/articles/paper", Some(change)).await.0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        call(&app, "DELETE", "/articles/paper/permanent", None)
            .await
            .0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        call(&app, "DELETE", "/articles/paper", None).await.0,
        StatusCode::OK
    );
    assert_eq!(
        call(&app, "GET", "/articles/paper", None).await.0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        call(&app, "GET", "/articles/paper/fulltext/raw", None)
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        call(&app, "GET", "/articles/paper/annotations", None)
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    let (_, trash) = call(&app, "GET", "/articles/trash", None).await;
    assert_eq!(trash["total"], 1);
    assert_eq!(trash["articles"][0]["article"]["title"], "User edit");
    assert_eq!(
        call(&app, "POST", "/articles/paper/restore", None).await.0,
        StatusCode::OK
    );
    let (_, restored) = call(&app, "GET", "/articles/paper", None).await;
    assert_eq!(restored["annotations"].as_array().unwrap().len(), 1);
    call(&app, "DELETE", "/articles/paper", None).await;
    assert_eq!(
        call(&app, "DELETE", "/articles/paper/permanent", None)
            .await
            .0,
        StatusCode::OK
    );
    assert_eq!(
        call(&app, "GET", "/articles/trash", None).await.1["total"],
        0
    );
}
#[tokio::test]
async fn manual_records_support_no_identifier_and_reject_invalid_or_duplicate_input() {
    let shared = BibShared::open_in_memory().await.unwrap();
    let app = api_server::bib::router(shared);
    let input =
        json!({"id":"","title":"Manual note","source":"manual","authors":[],"identifiers":[]});
    let (status, created) = call(&app, "POST", "/articles/manual", Some(input)).await;
    assert_eq!(status, StatusCode::CREATED);
    assert!(!created["article"]["id"].as_str().unwrap().is_empty());
    let input = json!({"id":"","title":"Saved webpage","identifiers":[{"kind":"other","value":"https://example.org/paper"}]});
    assert_eq!(
        call(&app, "POST", "/articles/manual", Some(input.clone()))
            .await
            .0,
        StatusCode::CREATED
    );
    assert_eq!(
        call(&app, "POST", "/articles/manual", Some(input)).await.0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        call(
            &app,
            "POST",
            "/articles/manual",
            Some(json!({"id":"","title":"  "}))
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        call(
            &app,
            "POST",
            "/articles/manual",
            Some(json!({"id":"","title":"valid","year":10000}))
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
}

#[tokio::test]
async fn attachment_bytes_survive_trash_and_restore_then_are_removed_on_purge() {
    let storage = std::sync::Arc::new(vfs::OpendalFileStorage::new_temp());
    let shared = BibShared::open_in_memory()
        .await
        .unwrap()
        .with_file_storage(storage.clone());
    shared
        .bib
        .upsert_article(&Article::new("paper", "Attachment record"))
        .await
        .unwrap();
    let app = api_server::bib::router(shared.clone());
    let mut stored = Vec::new();
    for (filename, contents) in [
        ("first.txt", "first attachment"),
        ("second.txt", "second attachment"),
    ] {
        let body = format!(
            "--boundary\r\ncontent-disposition: form-data; name=\"file\"; filename=\"{filename}\"\r\ncontent-type: text/plain\r\n\r\n{contents}\r\n--boundary--\r\n"
        );
        let request = Request::builder()
            .method("POST")
            .uri("/articles/paper/attachments")
            .header("content-type", "multipart/form-data; boundary=boundary")
            .body(Body::from(body))
            .unwrap();
        let response = app.clone().oneshot(request).await.unwrap();
        let status = response.status();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(
            status,
            StatusCode::CREATED,
            "{}",
            String::from_utf8_lossy(&bytes)
        );
        let payload: Value = serde_json::from_slice(&bytes).unwrap();
        stored.push((
            payload["attachment"]["key"].as_str().unwrap().to_owned(),
            payload["attachment"]["vfs_path"]
                .as_str()
                .unwrap()
                .to_owned(),
            contents,
        ));
    }
    call(&app, "DELETE", "/articles/paper", None).await;
    for (key, path, contents) in &stored {
        assert_eq!(
            call(
                &app,
                "GET",
                &format!("/articles/paper/attachments/{key}/raw"),
                None
            )
            .await
            .0,
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            bib_base::read_full(&storage, &bib_base::vfs_virtual_path(path).unwrap())
                .await
                .unwrap(),
            contents.as_bytes()
        );
    }
    call(&app, "POST", "/articles/paper/restore", None).await;
    for (key, _, contents) in &stored {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/articles/paper/attachments/{key}/raw"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.into_body().collect().await.unwrap().to_bytes(),
            contents.as_bytes()
        );
    }
    call(&app, "DELETE", "/articles/paper", None).await;
    assert_eq!(
        call(&app, "DELETE", "/articles/paper/permanent", None)
            .await
            .0,
        StatusCode::OK
    );
    for (_, path, _) in stored {
        assert!(
            bib_base::read_full(&storage, &bib_base::vfs_virtual_path(&path).unwrap())
                .await
                .is_err()
        );
    }
}
