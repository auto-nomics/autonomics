//! End-to-end integration tests that call the **real** Crossref REST API.
//!
//! These tests require network access and are therefore `#[ignore]` by default.
//! Run them explicitly with:
//!
//! ```bash
//! cargo test -p crossref --test e2e -- --ignored --nocapture
//! ```

use crossref::client::{ListQuery, WorksQuery};
use serial_test::serial;

use crossref::CrossrefClient;

/// A well-known DOI that is extremely unlikely to disappear.
const FAMOUS_DOI: &str = "10.1037/0003-066X.59.1.29"; // Baumeister — ego depletion


// ===========================================================================
// 1.  SDK: fetch a single work by DOI
// ===========================================================================

#[tokio::test]
#[serial]
#[ignore]
async fn sdk_works_by_doi() {
    let client = CrossrefClient::builder()
        .mailto("test@autonomics.dev")
        .build();

    let resp = client.works_by_doi(FAMOUS_DOI).await.expect("DOI fetch");

    let work = resp.message.expect("message");
    assert!(!work.doi.is_empty(), "DOI should be populated");
    assert!(!work.title.is_empty(), "title should be present");
    assert!(!work.author.is_empty(), "should have authors");
    assert_eq!(work.r#type, "journal-article");

    println!("✓ works_by_doi: {}", work.title_str());
    println!(
        "  Authors: {}",
        work.author
            .iter()
            .map(|a| a.display())
            .collect::<Vec<_>>()
            .join(", ")
    );
    println!("  Year: {:?}", work.year());
    println!("  Cited by: {}", work.is_referenced_by_count);
}

// ===========================================================================
// 2.  SDK: search /works with free-text query
// ===========================================================================

#[tokio::test]
#[serial]
#[ignore]
async fn sdk_works_search() {
    let client = CrossrefClient::with_mailto("test@autonomics.dev");

    let query = WorksQuery::new()
        .with_query("CRISPR gene editing")
        .with_rows(5)
        .with_sort("is-referenced-by-count")
        .with_order("desc");

    let resp = client.works(&query).await.expect("works search");
    let msg = &resp.message;

    assert!(msg.total_results > 0, "should find results for CRISPR");
    assert!(msg.items.len() <= 5, "should respect rows=5");
    assert!(!msg.items.is_empty(), "should have items");

    let first = &msg.items[0];
    assert!(!first.doi.is_empty(), "first result should have a DOI");

    println!(
        "✓ works search: {} total results, showing {}",
        msg.total_results,
        msg.items.len()
    );
    println!(
        "  Top result: {} (cited {}×)",
        first.title_str(),
        first.is_referenced_by_count
    );
}

// ===========================================================================
// 3.  SDK: StructuredSearch → WorksQuery translation + live query
// ===========================================================================

#[tokio::test]
#[serial]
#[ignore]
async fn sdk_structured_search_live() {
    use bib_types::query::{BoolOp, StructuredSearch, YearRange};

    let sq = StructuredSearch {
        keywords: Some(vec!["p53".into(), "cancer".into()]),
        keywords_op: Some(BoolOp::Or),
        year_range: Some(YearRange {
            from: 2023,
            to: 2024,
        }),
        ..Default::default()
    };

    let query = crossref::query::to_crossref_works_query(&sq).expect("translation");
    let query = query.with_rows(3);

    let client = CrossrefClient::with_mailto("test@autonomics.dev");
    let resp = client.works(&query).await.expect("structured search");

    let msg = &resp.message;
    assert!(msg.total_results > 0, "should find p53/cancer papers");

    // Verify year filter is respected (all results should be 2023 or 2024).
    for work in &msg.items {
        if let Some(y) = work.year() {
            assert!(
                (2023..=2024).contains(&y),
                "work year {y} outside filter range"
            );
        }
    }

    println!(
        "✓ structured search: {} results for p53 AND cancer (2023-2024)",
        msg.total_results
    );
    for w in &msg.items {
        println!("  - {} ({:?})", w.title_str(), w.year());
    }
}

// ===========================================================================
// 4.  SDK: works_agency — check DOI registration agency
// ===========================================================================

#[tokio::test]
#[serial]
#[ignore]
async fn sdk_works_agency() {
    let client = CrossrefClient::new();

    let resp = client
        .works_agency(FAMOUS_DOI)
        .await
        .expect("agency lookup");
    let info = resp.message.expect("agency message");

    assert_eq!(info.doi.to_lowercase(), FAMOUS_DOI.to_lowercase());
    assert_eq!(info.agency.id, "crossref");

    println!(
        "✓ works_agency: DOI {} → {} ({})",
        info.doi, info.agency.label, info.agency.id
    );
}

// ===========================================================================
// 5.  SDK: list members (publishers)
// ===========================================================================

#[tokio::test]
#[serial]
#[ignore]
async fn sdk_members_list() {
    let client = CrossrefClient::with_mailto("test@autonomics.dev");

    let list = ListQuery::new().with_query("Elsevier").with_rows(3);
    let resp = client.members(&list).await.expect("members search");

    let msg = &resp.message;
    assert!(!msg.items.is_empty(), "should find Elsevier");

    let first = &msg.items[0];
    // Some members may not report total_doi_count in all API versions.
    println!(
        "✓ members: {} (ID: {}, {} DOIs)",
        first.primary_name, first.id, first.total_doi_count
    );
}

// ===========================================================================
// 6.  SDK: list types
// ===========================================================================

#[tokio::test]
#[serial]
#[ignore]
async fn sdk_types_list() {
    let client = CrossrefClient::new();
    let resp = client.types().await.expect("types list");

    let msg = &resp.message;
    assert!(!msg.items.is_empty(), "should have work types");

    let ids: Vec<&str> = msg.items.iter().map(|t| t.id.as_str()).collect();
    assert!(
        ids.contains(&"journal-article"),
        "should include journal-article"
    );

    println!("✓ types: {} work types found", msg.items.len());
    for t in msg.items.iter().take(5) {
        println!("  - {} ({})", t.id, t.label);
    }
}

// ===========================================================================
// 7.  Convert: Work → bib_types::Article roundtrip
// ===========================================================================

#[tokio::test]
#[serial]
#[ignore]
async fn convert_work_to_article_live() {
    let client = CrossrefClient::with_mailto("test@autonomics.dev");
    let resp = client.works_by_doi(FAMOUS_DOI).await.expect("DOI fetch");
    let work = resp.message.expect("message");

    let article = crossref::work_to_article(&work);

    assert_eq!(article.source, bib_types::ArticleSource::CrossRef);
    assert!(!article.title.is_empty());
    assert!(!article.authors.is_empty());
    assert!(article.doi().is_some());

    println!("✓ work_to_article: {}", article.short_cite());
    println!("  DOI: {:?}", article.doi());
    println!("  Journal: {:?}", article.journal);
    println!("  Year: {:?}", article.year);
}

