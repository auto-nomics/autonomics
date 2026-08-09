//! Integration tests for the OpenAlex autocomplete endpoint.

mod common;

use openalex::OpenAlexClient;

#[tokio::test]
async fn autocomplete_works() {
    if !common::run_live() {
        return;
    }
    let client = common::client();
    let resp = client
        .autocomplete("works", "machine learning")
        .await
        .expect("autocomplete works should succeed");

    assert!(!resp.results.is_empty(), "should have suggestions");
    let first = &resp.results[0];
    assert!(first.display_name.is_some());
    assert!(first.id.is_some());
}

#[tokio::test]
async fn autocomplete_authors() {
    if !common::run_live() {
        return;
    }
    let client = common::client();
    let resp = client
        .autocomplete("authors", "Einstein")
        .await
        .expect("autocomplete authors should succeed");

    assert!(!resp.results.is_empty());
    // At least one should have "Einstein" in the name
    let has_einstein = resp
        .results
        .iter()
        .any(|r| r.display_name.as_deref().unwrap_or("").contains("Einstein"));
    assert!(has_einstein, "should find an Einstein");
}

#[tokio::test]
async fn autocomplete_sources() {
    if !common::run_live() {
        return;
    }
    let client = common::client();
    let resp = client
        .autocomplete("sources", "Nature")
        .await
        .expect("autocomplete sources should succeed");

    assert!(!resp.results.is_empty());
    let has_nature = resp
        .results
        .iter()
        .any(|r| r.display_name.as_deref().unwrap_or("").contains("Nature"));
    assert!(has_nature);
}

#[tokio::test]
async fn autocomplete_institutions() {
    if !common::run_live() {
        return;
    }
    let client = common::client();
    let resp = client
        .autocomplete("institutions", "Harvard")
        .await
        .expect("autocomplete institutions should succeed");

    assert!(!resp.results.is_empty());
    let has_harvard = resp
        .results
        .iter()
        .any(|r| r.display_name.as_deref().unwrap_or("").contains("Harvard"));
    assert!(has_harvard);
}

#[tokio::test]
async fn autocomplete_topics() {
    if !common::run_live() {
        return;
    }
    let client = common::client();
    let resp = client
        .autocomplete("topics", "cancer")
        .await
        .expect("autocomplete topics should succeed");

    assert!(!resp.results.is_empty());
}

#[tokio::test]
async fn autocomplete_funders() {
    if !common::run_live() {
        return;
    }
    let client = common::client();
    let resp = client
        .autocomplete("funders", "National")
        .await
        .expect("autocomplete funders should succeed");

    assert!(!resp.results.is_empty());
}
