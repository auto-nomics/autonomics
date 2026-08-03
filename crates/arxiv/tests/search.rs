use arxiv::{ArxivClient, types::SearchRequest};

type TestResult<T = ()> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;
mod common;

#[tokio::test]
#[ignore]
#[common::serial]
async fn search_basic_query() -> TestResult {
    let client = common::test_client();
    let resp = client
        .search(&SearchRequest::new("cat:cs.LG AND ti:transformer").max_results(5))
        .await?;

    assert!(resp.total_results > 0, "expected results for transformer search");
    assert!(!resp.entries.is_empty());
    assert!(resp.entries.len() <= 5);

    Ok(())
}

#[tokio::test]
#[ignore]
#[common::serial]
async fn search_with_max_results() -> TestResult {
    let client = common::test_client();
    common::rate_limit();
    let resp = client
        .search(&SearchRequest::new("machine learning").max_results(3))
        .await?;
    assert!(resp.entries.len() <= 3);

    Ok(())
}

#[tokio::test]
#[ignore]
#[common::serial]
async fn search_pagination() -> TestResult {
    let client = common::test_client();
    common::rate_limit();

    let req1 = SearchRequest::new("deep learning").max_results(5).start(0);
    let resp1 = client.search(&req1).await?;

    common::rate_limit();
    let req2 = SearchRequest::new("deep learning").max_results(5).start(5);
    let resp2 = client.search(&req2).await?;

    // Pages should not overlap
    let ids1: std::collections::HashSet<&str> =
        resp1.entries.iter().map(|e| e.arxiv_id.as_str()).collect();
    let ids2: std::collections::HashSet<&str> =
        resp2.entries.iter().map(|e| e.arxiv_id.as_str()).collect();
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
    common::rate_limit();
    let resp = client
        .search(
            &SearchRequest::new("quantum computing")
                .max_results(5)
                .sort_by(arxiv::types::SortBy::SubmittedDate),
        )
        .await?;
    assert_eq!(resp.entries.len(), 5);

    Ok(())
}

#[tokio::test]
#[ignore]
#[common::serial]
async fn search_entry_has_metadata() -> TestResult {
    let client = common::test_client();
    common::rate_limit();
    let resp = client
        .search(&SearchRequest::new("au:Hinton AND ti:neural").max_results(1))
        .await?;

    let entry = &resp.entries[0];
    assert!(!entry.title.is_empty());
    assert!(!entry.arxiv_id.is_empty());
    assert!(!entry.authors.is_empty());
    assert!(entry.summary.is_some());

    Ok(())
}
