use bioart::{BioArtClient, Error};
use serde_json::json;
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{body_string_contains, header, method, path},
};

async fn server() -> (MockServer, BioArtClient) {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/discover"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            r#"<script src="/_next/static/chunks/app/discover/page-test.js"></script>"#,
        ))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(path("/_next/static/chunks/app/discover/page-test.js")).respond_with(ResponseTemplate::new(200).set_body_string(r#"let a=(0,x.createServerReference)("abcdef0123",x.callServer,void 0,x.findSourceMapURL,"discoverSearch")"#)).expect(1).mount(&server).await;
    let client = BioArtClient::with_endpoint(&format!("{}/", server.uri())).unwrap();
    (server, client)
}
fn wire() -> String {
    format!(
        "0:{{\"a\":\"$@1\"}}\n1:{}\n",
        json!({"hits":{"found":1,"start":0,"hit":[{"fields":{"id":["228"],"title":["Human Heart"],"license":["Public Domain"],"creator":["Ryan Kissinger"],"collection":["NIAID"],"filesinfo":["All:11,12|SVG:11|PNG:12"],"description":["<p>Heart</p>"]}}]}})
    )
}
#[tokio::test]
async fn search_detail_and_bounded_download() {
    let (server, client) = server().await;
    Mock::given(method("POST"))
        .and(path("/discover"))
        .and(header("next-action", "abcdef0123"))
        .and(body_string_contains("type:bioart"))
        .respond_with(ResponseTemplate::new(200).set_body_string(wire()))
        .expect(2)
        .mount(&server)
        .await;
    let flight = json!([
        1,
        "1:[\"$\",\"x\",null,{\"filemapping\":{\"7\":{\"SVG\":11,\"PNG\":12}}}]\n"
    ]);
    Mock::given(path("/bioart/228"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(format!("<script>self.__next_f.push({flight})</script>")),
        )
        .mount(&server)
        .await;
    Mock::given(path("/api/bioarts/228/files/11"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "image/svg+xml")
                .set_body_string("<svg/>"),
        )
        .mount(&server)
        .await;
    let page = client.search("heart", 0, 10).await.unwrap();
    assert_eq!(page.total, 1);
    assert_eq!(page.assets[0].accession, "BIOART-000228");
    assert_eq!(page.assets[0].licenses, vec!["Public Domain"]);
    assert_eq!(page.assets[0].creators, vec!["Ryan Kissinger"]);
    assert_eq!(page.assets[0].files.len(), 2);
    let detail = client.detail(228).await.unwrap();
    assert_eq!(detail.representations[0].group_id, 7);
    let mut file = detail.asset.files[0].clone();
    file.url = "http://127.0.0.1:1/should-never-be-used".into();
    let download = client.download(&file, 100).await.unwrap();
    assert_eq!(download.bytes, b"<svg/>");
    assert_eq!(download.sha256.len(), 64);
    assert!(download.file.url.starts_with(&server.uri()));
    assert!(matches!(
        client.download(&file, 2).await,
        Err(Error::TooLarge(2))
    ));
    assert!(matches!(
        client.search("x", 0, 101).await,
        Err(Error::Invalid(_))
    ));
    // Discovery is cached across calls and clones, not hardcoded to a build ID.
    assert!(
        client
            .clone()
            .file(228, 11, "SVG")
            .url
            .ends_with("/api/bioarts/228/files/11")
    );
}
#[tokio::test]
async fn upstream_errors_and_protocol_drift_are_explicit() {
    let server = MockServer::start().await;
    let client = BioArtClient::with_endpoint(&format!("{}/", server.uri())).unwrap();
    Mock::given(path("/discover"))
        .respond_with(ResponseTemplate::new(200).set_body_string("<html>changed</html>"))
        .mount(&server)
        .await;
    assert!(matches!(
        client.search("heart", 0, 1).await,
        Err(Error::Protocol(_))
    ));
    Mock::given(path("/api/bioarts/228/files/11"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;
    assert!(matches!(
        client.download(&client.file(228, 11, "SVG"), 100).await,
        Err(Error::Http(_))
    ));
    assert!(BioArtClient::with_endpoint("file:///tmp/").is_err());
    assert!(BioArtClient::with_endpoint("https://user:pass@example.com/").is_err());
}
