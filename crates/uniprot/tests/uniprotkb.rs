//! Integration tests for the UniProtKB endpoints.
//!
//! These tests hit the live UniProt REST API. They require network access.
//! Run with: `cargo test -p uniprot --test uniprotkb -- --include-ignored`

mod common;

use uniprot::UniProtClient;
use uniprot::query::Query;
use uniprot::types::{Format, SearchRequest};

fn insulin_query() -> String {
    Query::new()
        .gene(["INS"])
        .organism_id(9606)
        .reviewed(true)
        .build()
        .expect("query builds")
}

#[tokio::test]
#[serial_test::serial]
#[ignore = "live UniProt API test"]
async fn search_returns_insulin_entry() {
    let client = common::client();
    let page = client
        .search(&SearchRequest::new(insulin_query()).size(5))
        .await
        .expect("search should succeed");

    // Note: gene:INS also matches the INSR2 readthrough entry (F8WCM5), so
    // assert presence of P01308 rather than an exact hit count.
    assert!(!page.results.is_empty());
    let entry = page
        .results
        .iter()
        .find(|e| e.primary_accession == "P01308")
        .expect("P01308 in results");
    assert_eq!(entry.uni_protkb_id, "INS_HUMAN");
    assert!(entry.is_reviewed());
    assert_eq!(entry.protein_name(), Some("Insulin"));
    assert_eq!(entry.sequence.length, Some(110));
}

#[tokio::test]
#[serial_test::serial]
#[ignore = "live UniProt API test"]
async fn search_paginates_with_cursor() {
    let client = common::client();
    let first = client
        .search(
            &SearchRequest::new("organism_id:9606 AND reviewed:true")
                .fields(["accession"])
                .size(2)
                .sort("accession asc"),
        )
        .await
        .expect("first page");

    assert_eq!(first.results.len(), 2);
    let total = first.total_results.expect("total header present");
    assert!(
        total > 20_000,
        "human Swiss-Prot has ~20k entries, got {total}"
    );

    let cursor = first.next_cursor.expect("next cursor present");
    let second = client
        .search(
            &SearchRequest::new("organism_id:9606 AND reviewed:true")
                .fields(["accession"])
                .size(2)
                .sort("accession asc")
                .cursor(cursor),
        )
        .await
        .expect("second page");

    assert_eq!(second.results.len(), 2);
    // Sorted ascending: no accession repeats across the page boundary.
    let page1: Vec<&str> = first
        .results
        .iter()
        .map(|e| e.primary_accession.as_str())
        .collect();
    for entry in &second.results {
        assert!(!page1.contains(&entry.primary_accession.as_str()));
    }
}

#[tokio::test]
#[serial_test::serial]
#[ignore = "live UniProt API test"]
async fn search_all_respects_cap() {
    let client = common::client();
    let entries = client
        .search_all(&SearchRequest::new("organism_id:9606 AND reviewed:true"), 5)
        .await
        .expect("search_all should succeed");
    assert_eq!(entries.len(), 5, "capped at max_results");
}

#[tokio::test]
#[serial_test::serial]
#[ignore = "live UniProt API test"]
async fn stream_returns_tsv_with_header() {
    let client = common::client();
    let tsv = client
        .stream(
            "accession:(P01308 OR P0DTC2)",
            Some(&["accession".into(), "id".into()]),
            Format::Tsv,
        )
        .await
        .expect("stream should succeed");

    let mut lines = tsv.lines();
    assert_eq!(lines.next(), Some("Entry\tEntry Name"));
    let mut ids: Vec<&str> = lines.map(|l| l.split('\t').nth(1).unwrap_or("")).collect();
    ids.sort_unstable();
    assert_eq!(ids, vec!["INS_HUMAN", "SPIKE_SARS2"]);
}

#[tokio::test]
#[serial_test::serial]
#[ignore = "live UniProt API test"]
async fn fasta_fetches_sequences() {
    let client = common::client();
    let fasta = client.fasta(&["P01308", "P0DTC2"]).await.expect("fasta");
    assert!(fasta.starts_with('>'));

    let insulin: String = fasta
        .split('>')
        .find(|chunk| chunk.contains("P01308"))
        .expect("insulin record present")
        .lines()
        .skip(1)
        .collect();
    assert_eq!(insulin.len(), 110, "human insulin is 110 aa");
}

#[tokio::test]
#[serial_test::serial]
#[ignore = "live UniProt API test"]
async fn entry_fetches_typed_and_raw() {
    let client = common::client();

    let entry = client.entry("P01308").await.expect("typed entry");
    assert_eq!(entry.uni_protkb_id, "INS_HUMAN");
    assert!(entry.function_text().is_some());

    let txt = client
        .entry_text("P01308", Format::Txt)
        .await
        .expect("flat file");
    assert!(txt.contains("ID   INS_HUMAN"));

    let gff = client.entry_text("P01308", Format::Gff).await.expect("gff");
    assert!(gff.contains("##gff-version"));
}

#[tokio::test]
#[serial_test::serial]
#[ignore = "live UniProt API test"]
async fn bad_query_returns_status_error() {
    let client = common::client();
    let err = client
        .search(&SearchRequest::new("NOT_A_FIELD:xyz"))
        .await
        .expect_err("invalid field should fail");
    assert!(matches!(
        err,
        uniprot::UniProtError::Status { status: 400, .. }
    ));
}

#[tokio::test]
#[serial_test::serial]
#[ignore = "live UniProt API test"]
async fn oversize_page_rejected_client_side() {
    let client = common::client();
    let err = client
        .search(&SearchRequest::new("insulin").size(501))
        .await
        .expect_err("size > 500 must fail");
    assert!(matches!(err, uniprot::UniProtError::Param(_)));
}
