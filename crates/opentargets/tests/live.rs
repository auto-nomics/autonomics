//! Live integration tests against the production Open Targets Platform API.
//!
//! These hit the real network and are `#[ignore]` by default:
//!
//! ```sh
//! cargo test -p opentargets -- --ignored --test-threads=1
//! ```

use opentargets::{OpenTargetsClient, Pagination};

fn client() -> OpenTargetsClient {
    OpenTargetsClient::new()
}

#[tokio::test]
#[ignore = "hits the live Open Targets API"]
async fn meta_works() {
    let meta = client().meta().await.unwrap();
    assert!(!meta.name.is_empty());
    assert!(!meta.api_version.x.is_empty());
}

#[tokio::test]
#[ignore = "hits the live Open Targets API"]
async fn target_brca1() {
    let t = client().target("ENSG00000012048").await.unwrap().unwrap();
    assert_eq!(t.id, "ENSG00000012048");
    assert_eq!(t.approved_symbol, "BRCA1");
    assert_eq!(t.genomic_location.chromosome, "17");
}

#[tokio::test]
#[ignore = "hits the live Open Targets API"]
async fn targets_batch() {
    let ts = client()
        .targets(&["ENSG00000012048", "ENSG00000139618"])
        .await
        .unwrap();
    assert_eq!(ts.len(), 2);
    assert!(ts.iter().any(|t| t.approved_symbol == "BRCA1"));
    assert!(ts.iter().any(|t| t.approved_symbol == "BRCA2"));
}

#[tokio::test]
#[ignore = "hits the live Open Targets API"]
async fn disease_alzheimer() {
    let d = client().disease("MONDO_0004975").await.unwrap().unwrap();
    assert_eq!(d.id, "MONDO_0004975");
    assert!(d.name.to_lowercase().contains("alzheimer"));
}

#[tokio::test]
#[ignore = "hits the live Open Targets API"]
async fn search_brca1() {
    let res = client()
        .search("BRCA1", Some(&["target"]), Some(Pagination::new(0, 5)))
        .await
        .unwrap();
    assert!(res.total > 0);
    assert!(res.hits.iter().any(|h| h.id == "ENSG00000012048"));
}

#[tokio::test]
#[ignore = "hits the live Open Targets API"]
async fn associated_diseases_page() {
    let page = client()
        .associated_diseases("ENSG00000012048", Pagination::new(0, 3), false, None)
        .await
        .unwrap();
    assert!(page.count > 100);
    assert_eq!(page.rows.len(), 3);
    // scores are sorted descending
    assert!(page.rows[0].score >= page.rows[1].score);
}

#[tokio::test]
#[ignore = "hits the live Open Targets API"]
async fn associated_diseases_all_small_filter() {
    // BRCA1 has >1000 associations; restrict with BFilter to keep the test
    // fast while still exercising pagination across multiple pages.
    let all = client()
        .associated_diseases_all_filtered("ENSG00000012048", false, Some("breast"))
        .await
        .unwrap();
    assert!(!all.is_empty());
    assert!(all.iter().all(|a| a.disease.name.to_lowercase().contains("breast")));
}

#[tokio::test]
#[ignore = "hits the live Open Targets API"]
async fn associated_targets_page() {
    let page = client()
        .associated_targets("MONDO_0004975", Pagination::new(0, 3), false, None)
        .await
        .unwrap();
    assert!(page.count > 0);
    assert!(!page.rows.is_empty());
}

#[tokio::test]
#[ignore = "hits the live Open Targets API"]
async fn drug_aspirin() {
    let d = client().drug("CHEMBL25").await.unwrap().unwrap();
    assert_eq!(d.id, "CHEMBL25");
}

#[tokio::test]
#[ignore = "hits the live Open Targets API"]
async fn study_lookup() {
    // A well-known GWAS Catalog study (crohn's disease).
    let s = client().study("GCST006131").await.unwrap();
    if let Some(s) = s {
        assert_eq!(s.id, "GCST006131");
        assert!(!s.trait_from_source.is_empty());
    }
}
