//! Live endpoint checks. Run explicitly with:
//! `cargo test -p reactome --test live -- --ignored`

use reactome::ReactomeClient;

#[tokio::test]
#[ignore = "live Reactome API test"]
async fn database_info() {
    let client = ReactomeClient::new();
    let info = client.database_info().await.unwrap();
    assert!(!info.name.is_empty(), "database name should not be empty");
    assert!(!info.version.is_empty(), "database version should not be empty");
}

#[tokio::test]
#[ignore = "live Reactome API test"]
async fn top_level_pathways() {
    let client = ReactomeClient::new();
    let pathways = client.top_level_pathways("Homo sapiens").await.unwrap();
    assert!(!pathways.is_empty(), "Homo sapiens should have top-level pathways");
    let p = &pathways[0];
    assert!(p.stable_id.is_some(), "pathway should have a stable ID");
    assert!(!p.display_name.is_empty());
}

#[tokio::test]
#[ignore = "live Reactome API test"]
async fn map_uniprot_to_pathways() {
    let client = ReactomeClient::new();
    let mapped = client.map_to_pathways("UniProt", "P04637").await.unwrap();
    assert!(!mapped.is_empty(), "TP53 (P04637) should map to pathways");
    assert!(mapped[0].stable_id.is_some());
}

#[tokio::test]
#[ignore = "live Reactome API test"]
async fn overrepresentation_analysis() {
    let client = ReactomeClient::new();
    let result = client
        .analyse_identifiers(
            &["TP53".to_string(), "BRCA1".to_string(), "EGFR".to_string(), "PTEN".to_string()],
            true,
        )
        .await
        .unwrap();
    assert!(!result.summary.token.is_empty(), "analysis should return a token");
    assert!(result.pathways_found > 0, "should find enriched pathways");
    let mapped = result.pathways_found + result.identifiers_not_found;
    assert!(mapped > 0, "should map at least one identifier");
}
