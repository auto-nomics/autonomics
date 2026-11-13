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
    let reused = client.clone().vector(&a.id).await.unwrap();
    assert_eq!(serde_json::to_value(&reused).unwrap(), serde_json::to_value(&vector).unwrap());
    assert_eq!(server.received_requests().await.unwrap().iter().filter(|r|r.url.path().ends_with(".svg")).count(),1);

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

#[tokio::test]
async fn short_molecule_queries_prioritize_names_and_tokens_before_substring_noise() {
    let server = MockServer::start().await;
    let names = ["Keras", "Barras", "Ras-GTP", "Ras", "RasGTP", "Ras_GDP"];
    let rows = names.iter().map(|name| json!({"name":name,"category":"Proteins","license":"cc-0","author":"Test Author"})).collect::<Vec<_>>();
    Mock::given(path("/icons/icons.json")).respond_with(ResponseTemplate::new(200).set_body_json(rows)).expect(1).mount(&server).await;
    Mock::given(path("/icons/authors.json")).respond_with(ResponseTemplate::new(200).set_body_json(json!({}))).mount(&server).await;
    let client = BioIconsClient::with_endpoint(&format!("{}/",server.uri())).unwrap();
    let page = client.search("RAS",0,100).await.unwrap();
    assert_eq!(page.total,6);
    assert_eq!(page.match_summary.exact_name,1);
    assert_eq!(page.match_summary.name_tokens,2);
    assert_eq!(page.match_summary.name_prefix,1);
    assert_eq!(page.match_summary.name_substring,2);
    assert_eq!(page.assets[0].name,"Ras");
    let positions=page.assets.iter().enumerate().map(|(i,a)|(a.name.as_str(),i)).collect::<std::collections::HashMap<_,_>>();
    for relevant in ["Ras-GTP","Ras_GDP"] {assert!(positions[relevant]<positions["RasGTP"]);}
    for noisy in ["Keras","Barras"] {assert!(positions["RasGTP"]<positions[noisy]);}
    let second=client.search("ras",2,2).await.unwrap();
    assert_eq!(second.total,page.total);
    assert_eq!(second.assets.iter().map(|a|&a.id).collect::<Vec<_>>(),page.assets[2..4].iter().map(|a|&a.id).collect::<Vec<_>>());
    for asset in &page.assets {assert_eq!(asset.license,"cc-0");assert_eq!(asset.catalog_sha256,page.assets[0].catalog_sha256);}
}

#[tokio::test]
async fn vector_failures_are_retried_then_success_is_shared_with_client_clones() {
    use std::sync::{Arc,atomic::{AtomicUsize,Ordering}};
    for first in [ResponseTemplate::new(503),ResponseTemplate::new(200).set_body_string("<svg><script/></svg>")] {
        let server=MockServer::start().await;
        Mock::given(path("/icons/icons.json")).respond_with(ResponseTemplate::new(200).set_body_json(json!([{"name":"test","category":"Cell","license":"cc-0","author":"Author"}]))).expect(1).mount(&server).await;
        Mock::given(path("/icons/authors.json")).respond_with(ResponseTemplate::new(200).set_body_json(json!({}))).expect(1).mount(&server).await;
        let requests=Arc::new(AtomicUsize::new(0));let count=requests.clone();
        let success=ResponseTemplate::new(200).set_body_string("<svg xmlns='http://www.w3.org/2000/svg' width='10' height='10'><rect width='10' height='10' fill='red'/></svg>");
        Mock::given(path("/icons/cc-0/Cell/Author/test.svg")).respond_with(move |_: &wiremock::Request|if count.fetch_add(1,Ordering::SeqCst)==0 {first.clone()} else {success.clone()}).expect(2).mount(&server).await;
        let client=BioIconsClient::with_endpoint(&format!("{}/",server.uri())).unwrap();let id=client.search("test",0,1).await.unwrap().assets[0].id.clone();
        assert!(client.vector(&id).await.is_err());let vector=client.vector(&id).await.unwrap();let reused=client.clone().vector(&id).await.unwrap();
        assert_eq!(vector.vector.sha256,reused.vector.sha256);assert_eq!(vector.vector.source_sha256,reused.vector.source_sha256);assert_eq!(requests.load(Ordering::SeqCst),2);
    }
}

#[tokio::test]
async fn exact_author_filter_applies_before_counts_ranking_and_pagination() {
    let server=MockServer::start().await;
    let rows=json!([
      {"name":"protein_a","category":"Protein","license":"cc-0","author":"One Author"},
      {"name":"protein_b","category":"Protein","license":"cc-by-4.0","author":"One Author"},
      {"name":"protein_c","category":"Protein","license":"cc-0","author":"One Author Studio"},
      {"name":"protein_One_Author","category":"Protein","license":"cc-0","author":"Other"}
    ]);
    Mock::given(path("/icons/icons.json")).respond_with(ResponseTemplate::new(200).set_body_json(rows)).expect(1).mount(&server).await;
    Mock::given(path("/icons/authors.json")).respond_with(ResponseTemplate::new(200).set_body_json(json!({}))).mount(&server).await;
    let client=BioIconsClient::with_endpoint(&format!("{}/",server.uri())).unwrap();
    for bad in [" ".to_owned(),"x".repeat(301)] {
        assert!(client.search_with_author("protein",0,12,Some(&bad)).await.is_err());
    }
    assert!(server.received_requests().await.unwrap().is_empty());
    let unfiltered=client.search("protein",0,12).await.unwrap();assert_eq!(unfiltered.total,4);assert!(unfiltered.author_filter.is_none());
    let page=client.search_with_author("protein",0,12,Some(" one AUTHOR ")).await.unwrap();
    assert_eq!(page.total,2);assert_eq!(page.author_filter.as_deref(),Some("one AUTHOR"));
    assert!(page.assets.iter().all(|a|a.author=="One Author"));
    assert_eq!(page.match_summary.name_tokens,2);
    let second=client.search_with_author("protein",1,1,Some("One Author")).await.unwrap();
    assert_eq!(second.total,2);assert_eq!(second.assets[0].id,page.assets[1].id);
    assert_eq!(second.assets[0].license,"cc-by-4.0");
    assert_eq!(second.assets[0].catalog_sha256,unfiltered.assets[0].catalog_sha256);
    let absent=client.search_with_author("protein",0,12,Some("Unknown Author")).await.unwrap();
    assert_eq!(absent.total,0);assert_eq!(absent.author_filter.as_deref(),Some("Unknown Author"));
}
