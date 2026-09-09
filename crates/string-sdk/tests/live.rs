use string_sdk::StringDbClient;
use string_sdk::request::{EnrichmentQuery, NetworkQuery, StringIdQuery};

#[tokio::test]
#[ignore = "requires network access and uses the public STRING service"]
async fn resolves_identifiers_without_an_api_key() {
    let client = StringDbClient::builder()
        .caller_identity("string-sdk-smoke-test")
        .build()
        .unwrap();

    let version = client.current_version().await.unwrap();
    assert!(!version.string_version.is_empty());
    assert!(version.stable_address.starts_with("https://"));

    let mappings = client
        .get_string_ids(&StringIdQuery::new(["TP53", "CDK2"]).species("9606"))
        .await
        .unwrap();
    assert_eq!(mappings.len(), 2);
    assert!(
        mappings
            .iter()
            .all(|row| row.string_id.starts_with("9606."))
    );
}

#[tokio::test]
#[ignore = "requires network access and uses the public STRING service"]
async fn retrieves_network_and_enrichment_models() {
    let client = StringDbClient::builder()
        .caller_identity("string-sdk-smoke-test")
        .build()
        .unwrap();
    let query = NetworkQuery::new(["TP53", "CDK2"]).species("9606");

    let interactions = client.network(&query).await.unwrap();
    assert!(!interactions.is_empty());
    assert!(
        interactions
            .iter()
            .all(|row| row.ncbi_taxon_id.as_u64() == Some(9606))
    );

    let enrichment = client
        .enrichment(&EnrichmentQuery::new(["TP53", "CDK2"]).species("9606"))
        .await
        .unwrap();
    assert!(!enrichment.is_empty());
    assert!(enrichment.iter().all(|row| row.fdr >= 0.0));

    let ppi = client
        .ppi_enrichment(&EnrichmentQuery::new(["TP53", "CDK2"]).species("9606"))
        .await
        .unwrap();
    assert_eq!(ppi.len(), 1);
}
