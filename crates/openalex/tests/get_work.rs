//! Integration tests for the OpenAlex single-entity endpoints.
//!
//! These tests hit the live OpenAlex API. They require network access.

mod common;

use openalex::OpenAlexClient;

#[tokio::test]
async fn get_work_by_openalex_id() {
    if !common::run_live() { return; }
    let client = common::client();
    let work = client
        .get_work("W2741809807")
        .await
        .expect("should fetch W2741809807");

    assert_eq!(work.id, "https://openalex.org/W2741809807");
    assert!(work.title.is_some(), "should have a title");
    assert!(work.doi.is_some(), "should have a DOI");
    assert!(work.publication_year.is_some());
    assert!(!work.authorships.is_empty(), "should have authors");
}

#[tokio::test]
async fn get_work_by_doi_shortcut() {
    if !common::run_live() { return; }
    let client = common::client();
    let work = client
        .get_work("doi:10.7717/peerj.4375")
        .await
        .expect("should fetch by DOI shortcut");

    assert!(work.doi.is_some());
    assert!(work
        .doi
        .as_ref()
        .unwrap()
        .contains("10.7717/peerj.4375"));
}

#[tokio::test]
async fn get_work_by_doi_full_url() {
    if !common::run_live() { return; }
    let client = common::client();
    let work = client
        .get_work("https://doi.org/10.7717/peerj.4375")
        .await
        .expect("should fetch by full DOI URL");

    assert!(work.title.is_some());
    assert_eq!(work.publication_year, Some(2018));
}

#[tokio::test]
async fn get_work_by_pmid() {
    if !common::run_live() { return; }
    let client = common::client();
    let work = client
        .get_work("pmid:29456894")
        .await
        .expect("should fetch by PMID");

    // This PMID corresponds to the same PeerJ article
    assert!(work.ids.pmid.is_some() || work.doi.is_some());
}

#[tokio::test]
async fn get_work_abstract_reconstruction() {
    if !common::run_live() { return; }
    let client = common::client();
    let work = client
        .get_work("W2741809807")
        .await
        .expect("should fetch work");

    let abs = work.abstract_text();
    assert!(abs.is_some(), "should have an abstract inverted index");
    let abs = abs.unwrap();
    assert!(abs.len() > 50, "abstract should be substantial");
}

#[tokio::test]
async fn get_work_not_found() {
    if !common::run_live() { return; }
    let client = common::client();
    let resp = client.get_work("W9999999999999").await;
    assert!(resp.is_err(), "nonexistent work should return error");
}

#[tokio::test]
async fn get_author_by_id() {
    if !common::run_live() { return; }
    let client = common::client();
    // Heather Piwowar — first author of the PeerJ article W2741809807
    let author = client
        .get_author("A5048491430")
        .await
        .expect("should fetch author");

    assert!(author.display_name.contains("Piwowar") || author.display_name.contains("Heather"));
    assert!(author.works_count > 0);
}

#[tokio::test]
async fn get_author_by_orcid() {
    if !common::run_live() { return; }
    let client = common::client();
    let author = client
        .get_author("https://orcid.org/0000-0003-1613-5981")
        .await
        .expect("should fetch by ORCID");

    assert!(!author.display_name.is_empty());
    assert!(author.works_count > 0);
}

#[tokio::test]
async fn list_sources_basic() {
    if !common::run_live() { return; }
    let client = common::client();
    let resp = client
        .list_sources(&openalex::ListParams::new().with_per_page(3))
        .await
        .expect("list_sources should succeed");

    assert!(resp.meta.count > 0);
    assert_eq!(resp.results.len(), 3);
}

#[tokio::test]
async fn list_institutions_basic() {
    if !common::run_live() { return; }
    let client = common::client();
    let resp = client
        .list_institutions(
            &openalex::ListParams::new()
                .with_filter("country_code:US")
                .with_per_page(3),
        )
        .await
        .expect("list_institutions should succeed");

    assert!(resp.meta.count > 0);
    for inst in &resp.results {
        assert_eq!(inst.country_code.as_deref(), Some("US"));
    }
}

#[tokio::test]
async fn list_topics_basic() {
    if !common::run_live() { return; }
    let client = common::client();
    let resp = client
        .list_topics(&openalex::ListParams::new().with_per_page(3))
        .await
        .expect("list_topics should succeed");

    assert!(resp.meta.count > 0);
    for t in &resp.results {
        assert!(!t.display_name.is_empty());
    }
}
