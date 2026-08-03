use bib_types::query::{BoolOp, StructuredSearch, YearRange};
use embase::{EmbaseClient, types::SearchRequest};

type TestResult<T = ()> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;
mod common;

#[tokio::test]
#[ignore]
#[common::serial]
async fn search_basic_query() -> TestResult {
    let client = common::test_client();
    let resp = client
        .search(&SearchRequest {
            query: "'CRISPR':ti,ab".into(),
            count: Some(5),
            ..Default::default()
        })
        .await?;

    let total: u64 = resp.total_results.parse()?;
    assert!(total > 0, "expected results for CRISPR search");
    assert!(!resp.entry.is_empty());
    assert!(resp.entry.len() <= 5);

    Ok(())
}

#[tokio::test]
#[ignore]
#[common::serial]
async fn search_structured_query() -> TestResult {
    let client = common::test_client();
    let sq = StructuredSearch {
        keywords: Some(vec!["CRISPR".into(), "gene editing".into()]),
        keywords_op: Some(BoolOp::Or),
        year_range: Some(YearRange { from: 2020, to: 2024 }),
        ..Default::default()
    };
    let query = embase::query::to_embase(&sq)?;
    let resp = client
        .search(&SearchRequest {
            query,
            count: Some(5),
            ..Default::default()
        })
        .await?;

    let total: u64 = resp.total_results.parse()?;
    assert!(total > 0, "expected results for structured query");

    Ok(())
}

#[tokio::test]
#[ignore]
#[common::serial]
async fn search_entry_has_metadata() -> TestResult {
    let client = common::test_client();
    let resp = client
        .search(&SearchRequest {
            query: "'cancer immunotherapy':ti,ab".into(),
            count: Some(1),
            ..Default::default()
        })
        .await?;

    let entry = &resp.entry[0];
    assert!(!entry.title.is_empty());

    Ok(())
}

#[tokio::test]
#[ignore]
#[common::serial]
async fn search_pagination() -> TestResult {
    let client = common::test_client();

    let req1 = SearchRequest {
        query: "'diabetes':ti,ab".into(),
        count: Some(5),
        start: Some(1),
        ..Default::default()
    };
    let resp1 = client.search(&req1).await?;
    assert_eq!(resp1.entry.len(), 5);

    let req2 = SearchRequest {
        query: "'diabetes':ti,ab".into(),
        count: Some(5),
        start: Some(6),
        ..Default::default()
    };
    let resp2 = client.search(&req2).await?;
    assert_eq!(resp2.entry.len(), 5);

    // Pages should not overlap (compare identifiers)
    let ids1: std::collections::HashSet<&str> =
        resp1.entry.iter().filter_map(|e| e.identifier.as_deref()).collect();
    let ids2: std::collections::HashSet<&str> =
        resp2.entry.iter().filter_map(|e| e.identifier.as_deref()).collect();
    assert!(
        ids1.intersection(&ids2).count() == 0,
        "pages should not overlap"
    );

    Ok(())
}

#[tokio::test]
#[ignore]
#[common::serial]
async fn search_sort_by_date() -> TestResult {
    let client = common::test_client();
    let resp = client
        .search(&SearchRequest {
            query: "'machine learning':ti,ab".into(),
            count: Some(5),
            sort: Some("entrydate".into()),
            ..Default::default()
        })
        .await?;
    assert_eq!(resp.entry.len(), 5);

    Ok(())
}

#[tokio::test]
#[ignore]
#[common::serial]
async fn search_to_articles_conversion() -> TestResult {
    use embase::convert::search_results_to_articles;

    let client = common::test_client();
    let resp = client
        .search(&SearchRequest {
            query: "'CRISPR':ti,ab".into(),
            count: Some(3),
            ..Default::default()
        })
        .await?;

    let articles = search_results_to_articles(&resp);
    assert_eq!(articles.len(), resp.entry.len());

    let a = &articles[0];
    assert!(!a.title.is_empty());
    assert_eq!(a.source, bib_types::ArticleSource::Embase);

    Ok(())
}
