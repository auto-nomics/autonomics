//! Integration tests for Europe PMC article, references, and citations endpoints.
//!
//! These tests hit the live Europe PMC API. They require network access.

mod common;

use europepmc::{EuropePmcClient, types::*};

#[tokio::test]
#[serial_test::serial]
#[ignore = "live Europe PMC API test"]
async fn article_by_pmid() {
    let client = common::client();
    let resp = client
        .article(Source::Med, "29867326", ResultType::Core)
        .await
        .expect("article retrieval should succeed");

    assert!(resp.result.is_some(), "article result should be present");
    let r = resp.result.as_ref().unwrap();
    assert!(!r.title.is_empty());
    assert_eq!(r.source, "MED");
    assert!(
        r.abstract_text.is_some(),
        "core article should have abstract"
    );
    assert!(
        r.author_list.is_some(),
        "core article should have author list"
    );
    assert!(
        r.pub_type_list.is_some(),
        "core article should have pub type list"
    );
}

#[tokio::test]
#[serial_test::serial]
#[ignore = "live Europe PMC API test"]
async fn article_by_doi_via_search() {
    // Europe PMC article endpoint uses source + id, not DOI directly.
    // We use search with DOI: prefix as a workaround.
    let client = common::client();
    let resp = client
        .search(&SearchRequest::new("DOI:10.1038/nature12345").result_type(ResultType::Lite))
        .await
        .expect("search by DOI should succeed");

    assert!(resp.hit_count >= 1);
}

#[tokio::test]
#[serial_test::serial]
#[ignore = "live Europe PMC API test"]
async fn references_retrieval() {
    let client = common::client();
    let resp = client
        .references("MED", "29867326", PageParams::new().page_size(5))
        .await
        .expect("references should succeed");

    assert!(resp.hit_count > 0, "expected non-zero reference count");
    let refs = resp
        .reference_list
        .as_ref()
        .expect("reference list should exist");
    assert!(!refs.references.is_empty(), "should have references");
    let r = &refs.references[0];
    assert!(r.title.is_some());
}

#[tokio::test]
#[serial_test::serial]
#[ignore = "live Europe PMC API test"]
async fn citations_retrieval() {
    let client = common::client();
    let resp = client
        .citations("MED", "29867326", PageParams::new().page_size(5))
        .await
        .expect("citations should succeed");

    assert!(resp.hit_count > 0, "expected non-zero citation count");
    let cites = resp
        .citation_list
        .as_ref()
        .expect("citation list should exist");
    assert!(!cites.citations.is_empty(), "should have citations");
    let c = &cites.citations[0];
    assert!(c.title.is_some());
}

#[tokio::test]
#[serial_test::serial]
#[ignore = "live Europe PMC API test"]
async fn profile_retrieval() {
    let client = common::client();
    let resp = client
        .profile("p53", "all")
        .await
        .expect("profile should succeed");

    let pl = resp
        .profile_list
        .as_ref()
        .expect("profile list should exist");
    assert!(
        pl.sources.is_some(),
        "profile should include source breakdown"
    );
    assert!(
        pl.pub_types.is_some(),
        "profile should include pub-type breakdown"
    );
}
