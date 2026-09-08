//! Integration tests for the taxonomy and proteomes endpoints.
//!
//! These tests hit the live UniProt REST API. They require network access.
//! Run with: `cargo test -p uniprot --test taxonomy_proteomes -- --include-ignored`

mod common;

use uniprot::UniProtClient;
use uniprot::types::SearchRequest;

// -----------------------------------------------------------------------
// Taxonomy
// -----------------------------------------------------------------------

#[tokio::test]
#[serial_test::serial]
#[ignore = "live UniProt API test"]
async fn taxonomy_lookup_by_id() {
    let client = common::client();
    let taxon = client.taxonomy_entry(9606).await.expect("human taxon");
    assert_eq!(taxon.scientific_name.as_deref(), Some("Homo sapiens"));
    assert_eq!(taxon.rank.as_deref(), Some("species"));
    let parent = taxon.parent.as_ref().expect("parent present");
    assert_eq!(parent.taxon_id, 9605);
    assert!(taxon.lineage.len() > 10, "full lineage to root");
}

#[tokio::test]
#[serial_test::serial]
#[ignore = "live UniProt API test"]
async fn taxonomy_search_by_free_text() {
    let client = common::client();
    let page = client
        .search_taxonomy(&SearchRequest::new("\"homo sapiens\"").size(3))
        .await
        .expect("taxonomy search");

    assert!(!page.results.is_empty());
    let found = page.results.iter().any(|t| t.taxon_id == 9606);
    assert!(found, "9606 should appear for 'homo sapiens'");
}

// -----------------------------------------------------------------------
// Proteomes
// -----------------------------------------------------------------------

#[tokio::test]
#[serial_test::serial]
#[ignore = "live UniProt API test"]
async fn proteome_entry_human() {
    let client = common::client();
    let proteome = client
        .proteome_entry("UP000005640")
        .await
        .expect("human proteome");
    assert_eq!(proteome.taxonomy.taxon_id, 9606);
    assert_eq!(
        proteome.taxonomy.scientific_name.as_deref(),
        Some("Homo sapiens")
    );
    assert!(proteome.protein_count.unwrap_or(0) > 50_000);
    assert!(
        proteome
            .proteome_statistics
            .reviewed_protein_count
            .unwrap_or(0)
            > 10_000
    );
}

#[tokio::test]
#[serial_test::serial]
#[ignore = "live UniProt API test"]
async fn proteomes_search_by_organism() {
    let client = common::client();
    let page = client
        .search_proteomes(&SearchRequest::new("organism_id:9606").size(5))
        .await
        .expect("proteomes search");

    assert!(!page.results.is_empty());
    assert!(
        page.results.iter().any(|p| p.id == "UP000005640"),
        "reference human proteome should be listed"
    );
}

#[tokio::test]
#[serial_test::serial]
#[ignore = "live UniProt API test"]
async fn invalid_upid_rejected_client_side() {
    let client = common::client();
    let err = client
        .proteome_entry("NOT_A_UPID")
        .await
        .expect_err("must fail");
    assert!(matches!(err, uniprot::UniProtError::Param(_)));
}
