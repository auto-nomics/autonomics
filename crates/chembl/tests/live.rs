//! Live integration checks. Run with:
//! `cargo test -p chembl --test live -- --ignored --test-threads=1`

use chembl::{ChEMBLClient, ResourceQuery};

#[tokio::test]
#[ignore = "hits the public ChEMBL API"]
async fn status_and_typed_records() {
    let client = ChEMBLClient::new();
    let status = client.status().await.unwrap();
    assert_eq!(status.status, "UP");
    assert!(status.activities > 0);

    let molecule = client.molecule("CHEMBL25").await.unwrap().unwrap();
    assert_eq!(molecule.molecule_chembl_id, "CHEMBL25");
    assert_eq!(molecule.pref_name.as_deref(), Some("ASPIRIN"));

    let target = client.target("CHEMBL2094253").await.unwrap().unwrap();
    assert_eq!(target.target_chembl_id, "CHEMBL2094253");
    assert!(!target.gene_symbols().is_empty());

    let missing = client.molecule("CHEMBL-not-real").await.unwrap();
    assert!(missing.is_none());
}

#[tokio::test]
#[ignore = "hits the public ChEMBL API"]
async fn activities_page() {
    let client = ChEMBLClient::new();
    let query = ResourceQuery::new().limit(5);
    let page = client
        .activities_for_molecule("CHEMBL25", &query)
        .await
        .unwrap();
    assert_eq!(page.records.len(), 5);
    assert!(page.page_meta.total_count > 0);
    assert!(
        page.records
            .iter()
            .all(|activity| activity.molecule_chembl_id == "CHEMBL25")
    );
}
