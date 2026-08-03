use arxiv::{ArxivClient, atom_to_articles, types::FetchRequest};

type TestResult<T = ()> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;
mod common;

/// Strip the `vN` version suffix from an arXiv ID (e.g. `"1706.03762v7"` → `"1706.03762"`).
fn strip_version(id: &str) -> &str {
    // Split at the first `v` that follows a digit, matching the `vN` pattern.
    if let Some(pos) = id.rfind('v') {
        let suffix = &id[pos + 1..];
        if !suffix.is_empty() && suffix.chars().all(|c| c.is_ascii_digit()) {
            return &id[..pos];
        }
    }
    id
}

#[tokio::test]
#[ignore]
#[common::serial]
async fn fetch_single_paper() -> TestResult {
    let client = common::test_client();
    let resp = client
        .fetch_by_id(&FetchRequest::new(common::ARXID_ATTENTION))
        .await?;

    assert_eq!(resp.entries.len(), 1);
    let entry = &resp.entries[0];
    // arXiv may append a version suffix (e.g. "1706.03762v7"); compare base IDs.
    assert_eq!(strip_version(&entry.arxiv_id), common::ARXID_ATTENTION);
    assert!(entry.title.contains("Attention"));
    assert!(!entry.authors.is_empty());
    assert!(entry.summary.is_some());
    assert!(entry.primary_category.is_some());

    Ok(())
}

#[tokio::test]
#[ignore]
#[common::serial]
async fn fetch_multiple_papers() -> TestResult {
    let client = common::test_client();
    common::rate_limit();
    let resp = client
        .fetch_by_id(&FetchRequest::new("1706.03762,1810.04805"))
        .await?;

    assert_eq!(resp.entries.len(), 2);

    Ok(())
}

#[tokio::test]
#[ignore]
#[common::serial]
async fn fetch_to_articles() -> TestResult {
    let client = common::test_client();
    common::rate_limit();
    let resp = client
        .fetch_by_id(&FetchRequest::new(common::ARXID_ATTENTION))
        .await?;

    let articles = atom_to_articles(&resp.entries);
    assert_eq!(articles.len(), 1);

    let a = &articles[0];
    let fetched_id = a.identifier(bib_types::IdKind::Arxiv).unwrap();
    assert_eq!(strip_version(fetched_id), common::ARXID_ATTENTION);
    assert!(!a.authors.is_empty());
    assert!(a.abstract_text.is_some());
    assert_eq!(a.source, bib_types::ArticleSource::Arxiv);

    Ok(())
}
