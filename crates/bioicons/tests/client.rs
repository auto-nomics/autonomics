use bioicons::BioIconsClient;
use serde_json::json;
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{method, path},
};
#[tokio::test]
async fn catalogue_search_retains_per_icon_licence_and_downloads_only_catalogue_paths() {
    let server = MockServer::start().await;
    Mock::given(method("GET")).and(path("/icons/icons.json")).respond_with(ResponseTemplate::new(200).set_body_json(json!([
        {"name":"test receptor","category":"Cell_membrane","license":"cc-by-4.0","author":"Test Author"},
        {"name":"other","category":"Lab","license":"unrecognized-license","author":"Unknown"}
    ]))).expect(1).mount(&server).await;
    Mock::given(path("/icons/authors.json"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"Test Author":"https://example.org/author"})),
        )
        .mount(&server)
        .await;
    Mock::given(path("/icons/cc-by-4.0/Cell_membrane/Test_Author/test%20receptor.svg")).respond_with(ResponseTemplate::new(200).set_body_string("<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 100 80'><rect width='100' height='80' fill='#112233'/></svg>")).mount(&server).await;
    let client = BioIconsClient::with_endpoint(&format!("{}/", server.uri())).unwrap();
    let page = client.search("test receptor", 0, 12).await.unwrap();
    assert_eq!(page.total, 1);
    let a = &page.assets[0];
    assert_eq!(a.license, "cc-by-4.0");
    assert!(a.license_url.as_ref().unwrap().contains("/by/4.0/"));
    assert_eq!(a.catalog_sha256.len(), 64);
    let vector = client.vector(&a.id).await.unwrap();
    assert_eq!(vector.vector.width, 100);
    assert_eq!(vector.vector.height, 80);
    assert!(
        vector
            .vector
            .data_url
            .starts_with("data:image/svg+xml;base64,")
    );
    assert!(
        vector
            .vector
            .preview_data_url
            .starts_with("data:image/png;base64,")
    );
    assert!(vector.vector.colors.contains(&"#112233".into()));
    let unknown = client.search("other", 0, 12).await.unwrap();
    assert!(unknown.assets[0].license_url.is_none());
    assert!(client.vector("../../etc/passwd").await.is_err());
}
#[tokio::test]
async fn catalogue_path_traversal_is_rejected() {
    let server = MockServer::start().await;
    Mock::given(path("/icons/icons.json"))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            json!([{"name":"../escape","category":"Cell","license":"cc-0","author":"Author"}]),
        ))
        .mount(&server)
        .await;
    Mock::given(path("/icons/authors.json"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
        .mount(&server)
        .await;
    assert!(
        BioIconsClient::with_endpoint(&format!("{}/", server.uri()))
            .unwrap()
            .search("escape", 0, 12)
            .await
            .is_err()
    );
}
