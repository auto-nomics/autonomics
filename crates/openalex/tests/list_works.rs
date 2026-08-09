//! Integration tests for the OpenAlex list/search/filter/group-by endpoints.
//!
//! These tests hit the live OpenAlex API. They require network access.
//! Set `OPENALEX_API_KEY` for higher rate limits.

mod common;

use openalex::{ListParams, OpenAlexClient};

#[tokio::test]
async fn list_works_basic() {
    if !common::run_live() { return; }
    let client = common::client();
    let resp = client
        .list_works(&ListParams::new().with_per_page(5))
        .await
        .expect("list_works should succeed");

    assert!(resp.meta.count > 0, "should have many works");
    assert_eq!(resp.results.len(), 5);
    let w = &resp.results[0];
    assert!(!w.id.is_empty(), "work should have an id");
}

#[tokio::test]
async fn list_works_with_filter() {
    if !common::run_live() { return; }
    let client = common::client();
    let resp = client
        .list_works(
            &ListParams::new()
                .with_filter("publication_year:2024,type:article,cited_by_count:>1000")
                .with_per_page(3)
                .with_sort("cited_by_count:desc"),
        )
        .await
        .expect("filtered list should succeed");

    assert!(resp.meta.count > 0);
    assert_eq!(resp.results.len(), 3);
    // Sorted by cited_by_count:desc → first should be >= second
    assert!(resp.results[0].cited_by_count >= resp.results[1].cited_by_count);
    // All should be 2024
    for w in &resp.results {
        assert_eq!(w.publication_year, Some(2024));
    }
}

#[tokio::test]
async fn search_works_keyword() {
    if !common::run_live() { return; }
    let client = common::client();
    let resp = client
        .search_works("CRISPR gene editing", Some(5))
        .await
        .expect("search should succeed");

    assert!(resp.meta.count > 0, "should find CRISPR works");
    assert!(resp.results.len() <= 5);
}

#[tokio::test]
async fn list_works_cursor_paging() {
    if !common::run_live() { return; }
    let client = common::client();

    let first = client
        .list_works(
            &ListParams::new()
                .with_filter("publication_year:2023")
                .with_per_page(2)
                .with_cursor("*"),
        )
        .await
        .expect("first cursor page");

    assert_eq!(first.results.len(), 2);
    let next_cursor = first
        .meta
        .next_cursor
        .clone()
        .expect("should have next_cursor");

    let second = client
        .list_works(
            &ListParams::new()
                .with_filter("publication_year:2023")
                .with_per_page(2)
                .with_cursor(&next_cursor),
        )
        .await
        .expect("second cursor page");

    assert_eq!(second.results.len(), 2);
    // Pages should have different IDs
    assert_ne!(first.results[0].id, second.results[0].id);
}

#[tokio::test]
async fn list_works_select_fields() {
    if !common::run_live() { return; }
    let client = common::client();
    let resp = client
        .list_works(
            &ListParams::new()
                .with_filter("publication_year:2024")
                .with_select("id,doi,title,publication_year,cited_by_count")
                .with_per_page(3),
        )
        .await
        .expect("select query should succeed");

    assert_eq!(resp.results.len(), 3);
    let w = &resp.results[0];
    assert!(!w.id.is_empty());
    assert!(w.publication_year.is_some());
}

#[tokio::test]
async fn list_works_group_by_type() {
    if !common::run_live() { return; }
    let client = common::client();
    let resp = client
        .list_works(
            &ListParams::new()
                .with_filter("publication_year:2024")
                .with_group_by("type"),
        )
        .await
        .expect("group_by should succeed");

    assert!(!resp.group_by.is_empty(), "should have aggregation results");
    let total: u64 = resp.group_by.iter().map(|g| g.count).sum();
    assert!(total > 1_000_000, "2024 should have millions of works");
    // article should be present — key is now a URL, key_display_name is "article"
    let article = resp
        .group_by
        .iter()
        .find(|g| g.key_display_name.as_deref() == Some("article"))
        .or_else(|| resp.group_by.iter().find(|g| g.key.contains("article")))
        .expect("should have article type");
    assert!(article.count > 1_000_000);
}

#[tokio::test]
async fn list_works_group_by_year() {
    if !common::run_live() { return; }
    let client = common::client();
    let resp = client
        .list_works(
            &ListParams::new()
                .with_filter("authorships.institutions.id:I136199984")
                .with_group_by("publication_year"),
        )
        .await
        .expect("group_by year should succeed");

    assert!(!resp.group_by.is_empty());
    // Harvard (I136335617) should have works spanning many years
    assert!(resp.group_by.len() > 10);
}

#[tokio::test]
async fn list_works_or_filter() {
    if !common::run_live() { return; }
    let client = common::client();
    let resp = client
        .list_works(
            &ListParams::new()
                .with_filter("publication_year:2024,type:article|book-chapter")
                .with_per_page(5),
        )
        .await
        .expect("OR filter should succeed");

    assert!(resp.meta.count > 0);
    for w in &resp.results {
        let t = w.type_.as_deref().unwrap_or("");
        assert!(
            t == "article" || t == "book-chapter",
            "type should be article or book-chapter, got '{t}'"
        );
    }
}

#[tokio::test]
async fn list_works_not_filter() {
    if !common::run_live() { return; }
    let client = common::client();
    let resp = client
        .list_works(
            &ListParams::new()
                .with_filter("publication_year:2024,type:!paratext")
                .with_per_page(3),
        )
        .await
        .expect("NOT filter should succeed");

    assert!(resp.meta.count > 0);
    for w in &resp.results {
        assert_ne!(
            w.type_.as_deref(),
            Some("paratext"),
            "should not include paratext"
        );
    }
}

#[tokio::test]
async fn list_works_all_auto_page() {
    if !common::run_live() { return; }
    let client = common::client();
    // Narrow filter so we get a manageable number
    let works = client
        .list_works_all(
            &ListParams::new()
                .with_filter("publication_year:2024,type:article,cited_by_count:>10000"),
        )
        .await
        .expect("list_works_all should succeed");

    // There should be a small number of articles with >10k citations in 2024
    assert!(!works.is_empty(), "should find some highly cited works");
    assert!(works.len() < 500, "should not be too many (sanity check), got {}", works.len());
}

#[tokio::test]
async fn error_on_nonexistent_filter_field() {
    if !common::run_live() { return; }
    let client = common::client();
    let resp = client
        .list_works(
            &ListParams::new().with_filter("nonexistent_field:abc"),
        )
        .await;

    assert!(resp.is_err(), "bad filter should return an error");
}

#[tokio::test]
async fn work_has_title_and_ids() {
    if !common::run_live() { return; }
    let client = common::client();
    let resp = client
        .list_works(&ListParams::new().with_per_page(1))
        .await
        .expect("should succeed");

    let w = &resp.results[0];
    assert!(w.title_or_name().is_some(), "should have a title/display_name");
    assert!(w.id.starts_with("https://openalex.org/W"), "id should be an OpenAlex work URL");
    // ids.mag should be a string (not u64)
    if let Some(ref mag) = w.ids.mag {
        assert!(mag.parse::<u64>().is_ok(), "mag should be a numeric string, got '{mag}'");
    }
}
