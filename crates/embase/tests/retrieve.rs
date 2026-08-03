use embase::EmbaseClient;

type TestResult<T = ()> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;
mod common;

/// Retrieve a well-known article by DOI.
/// Uses the "Attention Is All You Need" paper as a stable test fixture.
#[tokio::test]
#[ignore]
#[common::serial]
async fn retrieve_by_doi() -> TestResult {
    let client = common::test_client();
    let resp = client
        .retrieve_by_doi("10.1016/j.cell.2024.01.001")
        .await?;

    assert!(!resp.entry.title.is_empty());

    Ok(())
}

#[tokio::test]
#[ignore]
#[common::serial]
async fn retrieve_by_pmid() -> TestResult {
    let client = common::test_client();
    // PMID for a stable, well-known article
    let resp = client.retrieve_by_pmid("36623027").await?;

    assert!(!resp.entry.title.is_empty());

    Ok(())
}
