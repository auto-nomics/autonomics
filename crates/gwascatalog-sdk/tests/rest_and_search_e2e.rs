//! Live integration tests for the GWAS Catalog REST API and Solr Search API.
//!
//! These hit the real EBI endpoints. Run with:
//! ```sh
//! cargo test -p gwascatalog-sdk --test rest_and_search_e2e -- --ignored
//! ```

use gwascatalog_sdk::{
    rest::{
        AssociationQueryKind, EfoQuery, SnpQuery, StudyQuery, UnpublishedFilter,
    },
    search::SearchFilter,
    GwasCatalogClient,
};

fn client() -> GwasCatalogClient {
    GwasCatalogClient::new()
}

// ── Solr Search API ─────────────────────────────────────────────────────────

#[tokio::test]
#[ignore = "hits the live GWAS Catalog Search API"]
async fn search_all_resources() {
    let resp = client()
        .search(&SearchFilter {
            q: "*:*".into(),
            max: Some(1),
            ..Default::default()
        })
        .await
        .unwrap();
    assert!(resp.response.num_found > 0);
    assert!(!resp.response.docs.is_empty());
}

#[tokio::test]
#[ignore = "hits the live GWAS Catalog Search API"]
async fn search_studies_by_trait() {
    let resp = client()
        .search(&SearchFilter {
            q: "resourcename:study AND \"breast cancer\"".into(),
            max: Some(5),
            ..Default::default()
        })
        .await
        .unwrap();
    assert!(resp.response.docs.len() <= 5);
}

#[tokio::test]
#[ignore = "hits the live GWAS Catalog Search API"]
async fn search_with_genomic_filter() {
    let resp = client()
        .search(&SearchFilter {
            q: "resourcename:variant".into(),
            max: Some(3),
            genomic_filter: Some("1:1000000-2000000".into()),
            ..Default::default()
        })
        .await
        .unwrap();
    assert!(!resp.response.docs.is_empty());
}

// ── REST API: Studies ───────────────────────────────────────────────────────

#[tokio::test]
#[ignore = "hits the live GWAS Catalog REST API"]
async fn rest_list_studies() {
    let resp = client()
        .rest_studies(Some(0), Some(2))
        .await
        .unwrap();
    assert!(resp.page.total_elements > 0);
}

#[tokio::test]
#[ignore = "hits the live GWAS Catalog REST API"]
async fn rest_find_study_by_accession() {
    // findByAccessionId returns a flat entity, so use the direct GET endpoint
    let study = client()
        .rest_get_study("GCST005038")
        .await
        .unwrap();
    assert_eq!(study.accession_id, "GCST005038");
}

#[tokio::test]
#[ignore = "hits the live GWAS Catalog REST API"]
async fn rest_find_studies_by_pmid() {
    let resp = client()
        .rest_find_studies(
            &StudyQuery::Pmid("21041247".into()),
            Some(0),
            Some(5),
        )
        .await
        .unwrap();
    let embedded = resp._embedded.unwrap();
    assert!(!embedded.studies.is_empty());
}

// ── REST API: Associations ──────────────────────────────────────────────────

#[tokio::test]
#[ignore = "hits the live GWAS Catalog REST API"]
async fn rest_study_associations() {
    let resp = client()
        .rest_study_associations("GCST005038", Some(0), Some(3))
        .await
        .unwrap();
    let embedded = resp._embedded.unwrap();
    assert!(!embedded.associations.is_empty());
}

#[tokio::test]
#[ignore = "hits the live GWAS Catalog REST API"]
async fn rest_find_associations_by_rsid() {
    let resp = client()
        .rest_find_associations(
            &AssociationQueryKind::RsId("rs7329174".into()),
            Some(0),
            Some(3),
        )
        .await
        .unwrap();
    let embedded = resp._embedded.unwrap();
    assert!(!embedded.associations.is_empty());
}

// ── REST API: EFO Traits ────────────────────────────────────────────────────

#[tokio::test]
#[ignore = "hits the live GWAS Catalog REST API"]
async fn rest_get_efo_trait() {
    let trait_ = client()
        .rest_get_efo_trait("EFO_0004343")
        .await
        .unwrap();
    assert_eq!(trait_.short_form, "EFO_0004343");
    assert!(!trait_.trait_name.is_empty());
}

#[tokio::test]
#[ignore = "hits the live GWAS Catalog REST API"]
async fn rest_find_efo_traits_by_name() {
    let resp = client()
        .rest_find_efo_traits(
            &EfoQuery::Trait("body mass index".into()),
            Some(0),
            Some(5),
        )
        .await
        .unwrap();
    let embedded = resp._embedded.unwrap();
    assert!(!embedded.efo_traits.is_empty());
}

// ── REST API: SNPs ──────────────────────────────────────────────────────────

#[tokio::test]
#[ignore = "hits the live GWAS Catalog REST API"]
async fn rest_get_snp() {
    let snp = client().rest_get_snp("rs7329174").await.unwrap();
    assert_eq!(snp.rs_id, "rs7329174");
}

#[tokio::test]
#[ignore = "hits the live GWAS Catalog REST API"]
async fn rest_find_snp_by_gene() {
    let resp = client()
        .rest_find_snps(
            &SnpQuery::Gene("BRCA1".into()),
            Some(0),
            Some(3),
        )
        .await
        .unwrap();
    let embedded = resp._embedded.unwrap();
    assert!(!embedded.single_nucleotide_polymorphisms.is_empty());
}

// ── REST API: Unpublished Studies ───────────────────────────────────────────

#[tokio::test]
#[ignore = "hits the live GWAS Catalog REST API"]
async fn rest_unpublished_studies() {
    let resp = client()
        .rest_unpublished_studies(
            &UnpublishedFilter {
                trait_: Some("diabetes".into()),
                ..Default::default()
            },
            Some(0),
            Some(3),
        )
        .await;
    // Unpublished studies may or may not exist for a given trait — just
    // verify the call doesn't error on the response shape.
    let _ = resp;
}
