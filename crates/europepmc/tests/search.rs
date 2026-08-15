//! Integration tests for the Europe PMC search endpoint.
//!
//! These tests hit the live Europe PMC API. They require network access.
//! Run with: `cargo test -p europepmc --test search -- --include-ignored`

mod common;

use bib_types::query::{BoolOp, StructuredSearch, YearRange};
use europepmc::{EuropePmcClient, query::to_europepmc, types::*};

#[tokio::test]
#[serial_test::serial]
#[ignore = "live Europe PMC API test"]
async fn search_basic_query() {
    let client = common::client();
    let resp = client
        .search(&SearchRequest::new("p53").page_size(3))
        .await
        .expect("search should succeed");

    assert!(resp.hit_count > 0, "expected non-zero hit count");
    assert_eq!(resp.result_list.results.len(), 3);
    let r = &resp.result_list.results[0];
    assert!(!r.title.is_empty(), "title should not be empty");
    assert!(!r.source.is_empty());
    assert!(!r.id.is_empty());
}

#[tokio::test]
#[serial_test::serial]
#[ignore = "live Europe PMC API test"]
async fn search_core_has_abstract_and_authors() {
    let client = common::client();
    let resp = client
        .search(
            &SearchRequest::new("TITLE:p53 AND PUB_TYPE:Review")
                .result_type(ResultType::Core)
                .page_size(1),
        )
        .await
        .expect("core search should succeed");

    assert!(resp.hit_count > 0);
    let r = &resp.result_list.results[0];
    assert!(r.abstract_text.is_some(), "core should have abstract");
    assert!(
        r.author_list.is_some(),
        "core should have structured author list"
    );
    let authors = r.author_list.as_ref().unwrap();
    assert!(
        !authors.authors.is_empty(),
        "should have at least one author"
    );
}

#[tokio::test]
#[serial_test::serial]
#[ignore = "live Europe PMC API test"]
async fn search_with_cursor_pagination() {
    let client = common::client();
    let first = client
        .search(&SearchRequest::new("cancer").page_size(2))
        .await
        .expect("first page");

    assert!(first.next_cursor_mark.is_some());

    let second = client
        .search(
            &SearchRequest::new("cancer")
                .page_size(2)
                .cursor_mark(first.next_cursor_mark.unwrap()),
        )
        .await
        .expect("second page");

    // The second page should return different IDs than the first.
    let first_ids: Vec<&str> = first
        .result_list
        .results
        .iter()
        .map(|r| r.id.as_str())
        .collect();
    let second_ids: Vec<&str> = second
        .result_list
        .results
        .iter()
        .map(|r| r.id.as_str())
        .collect();
    assert!(
        first_ids != second_ids,
        "second page should have different IDs"
    );
}

#[tokio::test]
#[serial_test::serial]
#[ignore = "live Europe PMC API test"]
async fn search_structured_query() {
    let sq = StructuredSearch {
        keywords: Some(vec!["p53".into(), "cancer".into()]),
        keywords_op: Some(BoolOp::And),
        year_range: Some(YearRange {
            from: 2023,
            to: 2024,
        }),
        ..Default::default()
    };
    let query = to_europepmc(&sq).unwrap();

    let client = common::client();
    let resp = client
        .search(&SearchRequest::new(&query).page_size(1))
        .await
        .expect("structured search should succeed");

    assert!(resp.hit_count > 0, "expected non-zero hit count");
}

#[tokio::test]
#[serial_test::serial]
#[ignore = "live Europe PMC API test"]
async fn search_idlist_result_type() {
    let client = common::client();
    let resp = client
        .search(
            &SearchRequest::new("DOI:10.1007/s11033-026-12527-x").result_type(ResultType::Idlist),
        )
        .await
        .expect("idlist search should succeed");

    assert!(resp.hit_count >= 1);
    let r = &resp.result_list.results[0];
    assert!(!r.source.is_empty());
    assert!(!r.id.is_empty());
}

#[tokio::test]
#[serial_test::serial]
#[ignore = "live Europe PMC API test"]
async fn search_sort_by_cited() {
    let client = common::client();
    let resp = client
        .search(
            &SearchRequest::new("TITLE:p53")
                .page_size(2)
                .sort("CITED desc"),
        )
        .await
        .expect("sorted search should succeed");

    // The most cited should come first.
    let counts: Vec<u64> = resp
        .result_list
        .results
        .iter()
        .map(|r| r.cited_by_count.unwrap_or(0))
        .collect();
    if counts.len() >= 2 {
        assert!(
            counts[0] >= counts[1],
            "results should be sorted by cited desc"
        );
    }
}
