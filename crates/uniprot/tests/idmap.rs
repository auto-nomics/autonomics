//! Integration tests for the ID mapping endpoints.
//!
//! These tests hit the live UniProt REST API. They require network access.
//! Run with: `cargo test -p uniprot --test idmap -- --include-ignored`

mod common;

use uniprot::UniProtClient;

#[tokio::test]
#[serial_test::serial]
#[ignore = "live UniProt API test (server-side job, polls up to ~1 min)"]
async fn maps_gene_names_to_uniprotkb() {
    let client = common::client();
    let results = client
        .map_ids_json("Gene_Name", "UniProtKB", &["INS".to_owned()])
        .await
        .expect("mapping should finish");

    assert!(!results.results.is_empty());
    // Gene_Name=INS matches INS across all organisms; human must be present.
    let accessions: Vec<&str> = results
        .results
        .iter()
        .filter_map(|r| r.to.as_ref().map(|e| e.primary_accession.as_str()))
        .collect();
    assert!(
        accessions.contains(&"P01308"),
        "P01308 missing from {accessions:?}"
    );
    let human = results
        .results
        .iter()
        .find_map(|r| r.to.as_ref().filter(|e| e.primary_accession == "P01308"))
        .expect("human insulin entry");
    assert_eq!(human.uni_protkb_id, "INS_HUMAN");
}

#[tokio::test]
#[serial_test::serial]
#[ignore = "live UniProt API test (server-side job, polls up to ~1 min)"]
async fn map_ids_tsv_has_from_column() {
    let client = common::client();
    let tsv = client
        .map_ids(
            "UniProtKB_AC-ID",
            "UniProtKB",
            &["P01308".to_owned()],
            Some(&["accession".into(), "id".into()]),
        )
        .await
        .expect("tsv mapping");

    let mut lines = tsv.lines();
    let header = lines.next().expect("header row");
    assert!(header.starts_with("From\t"), "header was: {header}");
    let row = lines.next().expect("data row");
    let cols: Vec<&str> = row.split('\t').collect();
    assert_eq!(cols[0], "P01308");
}

#[tokio::test]
#[serial_test::serial]
#[ignore = "live UniProt API test"]
async fn submit_requires_ids() {
    let client = common::client();
    let err = client
        .submit_id_mapping("Gene_Name", "UniProtKB", &[])
        .await
        .expect_err("empty ids must fail");
    assert!(matches!(err, uniprot::UniProtError::Param(_)));
}

#[tokio::test]
#[serial_test::serial]
#[ignore = "live UniProt API test"]
async fn unknown_source_db_returns_error() {
    let client = common::client();
    let err = client
        .submit_id_mapping("Not_A_Database", "UniProtKB", &["INS".to_owned()])
        .await
        .expect_err("invalid source database must fail");
    let msg = format!("{err}");
    assert!(msg.to_lowercase().contains("invalid"), "was: {msg}");
}
