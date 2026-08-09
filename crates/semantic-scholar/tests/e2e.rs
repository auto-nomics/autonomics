//! End-to-end integration tests for the Semantic Scholar SDK.
//!
//! These tests hit the **live** Semantic Scholar Academic Graph API.
//! They require network access and share the public rate-limit pool.
//!
//! Tests are **serialised** (`serial_test::serial`) to avoid 429 rate
//! limiting on the shared unauthenticated pool.
//!
//! Run with:
//! ```sh
//! cargo test -p semantic-scholar --test e2e -- --include-ignored
//! ```
//!
//! For higher rate limits, set `S2_API_KEY`:
//! ```sh
//! S2_API_KEY=your_key cargo test -p semantic-scholar --test e2e -- --include-ignored
//! ```

mod common;

use bib_types::query::{BoolOp, StructuredSearch, YearRange};
use bib_types::{ArticleSource, IdKind};
use semantic_scholar::{PaperSearchFilter, paper_to_article, query::to_s2};

use serial_test::serial;

type TestResult<T = ()> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// Helper: sleep to avoid rate limiting between tests.
async fn throttle() {
    tokio::time::sleep(std::time::Duration::from_millis(1200)).await;
}

// ===========================================================================
// Paper search
// ===========================================================================

#[tokio::test]
#[ignore]
#[serial]
async fn search_basic_query() -> TestResult {
    let client = common::client();
    let resp = client.search_paper("covid vaccination", 5, None).await?;

    assert!(resp.total > 0, "expected non-zero total");
    assert!(!resp.data.is_empty(), "expected at least one result");
    assert!(resp.data.len() <= 5);

    let p = &resp.data[0];
    assert!(!p.paper_id.is_empty(), "paperId should be non-empty");
    assert!(p.title.is_some(), "title should be present");

    Ok(())
}

#[tokio::test]
#[ignore]
#[serial]
async fn search_returns_rich_metadata() -> TestResult {
    throttle().await;
    let client = common::client();
    let resp = client.search_paper("deep learning genomics", 3, None).await?;

    if resp.data.is_empty() {
        return Ok(()); // API returned nothing; skip
    }
    let p = &resp.data[0];
    assert!(p.year.is_some(), "year should be present with default fields");
    assert!(
        !p.authors.is_empty(),
        "authors should be present with default fields"
    );
    assert!(
        p.citation_count.is_some(),
        "citationCount should be present"
    );

    Ok(())
}

#[tokio::test]
#[ignore]
#[serial]
async fn search_with_year_filter() -> TestResult {
    throttle().await;
    let client = common::client();
    let filter = PaperSearchFilter {
        year: Some("2023-2024".into()),
        ..Default::default()
    };
    let resp = client
        .search_paper_filtered("cancer immunotherapy", 5, 0, &filter, None)
        .await?;

    for p in &resp.data {
        if let Some(y) = p.year {
            assert!(
                (2023..=2024).contains(&y),
                "paper year {y} should be in [2023, 2024]"
            );
        }
    }

    Ok(())
}

#[tokio::test]
#[ignore]
#[serial]
async fn search_with_open_access_filter() -> TestResult {
    throttle().await;
    let client = common::client();
    let filter = PaperSearchFilter {
        open_access_pdf: true,
        ..Default::default()
    };
    let resp = client
        .search_paper_filtered("CRISPR", 5, 0, &filter, None)
        .await?;

    for p in &resp.data {
        assert!(
            p.is_open_access == Some(true),
            "paper should be open access when filter is set"
        );
    }

    Ok(())
}

#[tokio::test]
#[ignore]
#[serial]
async fn search_with_min_citation_count() -> TestResult {
    throttle().await;
    let client = common::client();
    let filter = PaperSearchFilter {
        min_citation_count: Some(100),
        ..Default::default()
    };
    let resp = client
        .search_paper_filtered("transformer neural network", 5, 0, &filter, None)
        .await?;

    for p in &resp.data {
        let cc = p.citation_count.unwrap_or(0);
        assert!(
            cc >= 100,
            "citation_count {cc} should be >= 100 (minCitationCount filter)"
        );
    }

    Ok(())
}

#[tokio::test]
#[ignore]
#[serial]
async fn search_structured_query() -> TestResult {
    throttle().await;
    let sq = StructuredSearch {
        keywords: Some(vec!["p53".into(), "cancer".into()]),
        keywords_op: Some(BoolOp::And),
        year_range: Some(YearRange { from: 2022, to: 2024 }),
        ..Default::default()
    };
    let parts = to_s2(&sq)?;

    let client = common::client();
    let resp = client
        .search_paper_filtered(&parts.query, 5, 0, &parts.filter, None)
        .await?;

    assert!(resp.total > 0, "expected results for structured query");
    assert!(parts.filter.year.as_deref() == Some("2022-2024"));

    Ok(())
}

#[tokio::test]
#[ignore]
#[serial]
async fn search_pagination() -> TestResult {
    throttle().await;
    let client = common::client();
    let page1 = client.search_paper("machine learning", 5, None).await?;
    assert!(page1.next.is_some(), "should have next offset");

    let offset = page1.next.unwrap() as u32;
    throttle().await;
    let page2 = client
        .search_paper_filtered(
            "machine learning",
            5,
            offset,
            &PaperSearchFilter::default(),
            None,
        )
        .await?;

    let ids1: std::collections::HashSet<&str> =
        page1.data.iter().map(|p| p.paper_id.as_str()).collect();
    let ids2: std::collections::HashSet<&str> =
        page2.data.iter().map(|p| p.paper_id.as_str()).collect();
    assert!(
        ids1.intersection(&ids2).count() == 0,
        "pages should not overlap"
    );

    Ok(())
}

// ===========================================================================
// Paper detail
// ===========================================================================

#[tokio::test]
#[ignore]
#[serial]
async fn get_paper_by_sha() -> TestResult {
    throttle().await;
    let client = common::client();
    let search = client.search_paper("attention is all you need", 1, None).await?;
    if search.data.is_empty() {
        return Ok(());
    }
    let paper_id = search.data[0].paper_id.clone();

    throttle().await;
    let paper = client.get_paper(&paper_id, None).await?;
    assert_eq!(paper.paper_id, paper_id);
    assert!(paper.title.is_some());

    Ok(())
}

#[tokio::test]
#[ignore]
#[serial]
async fn get_paper_by_arxiv_id() -> TestResult {
    throttle().await;
    let client = common::client();
    let paper = client.get_paper("ARXIV:1706.03762", None).await?;
    assert!(paper.title.is_some());
    assert!(
        paper
            .title
            .as_ref()
            .unwrap()
            .to_lowercase()
            .contains("attention"),
        "title should contain 'attention', got: {:?}",
        paper.title
    );

    Ok(())
}

#[tokio::test]
#[ignore]
#[serial]
async fn get_papers_batch() -> TestResult {
    throttle().await;
    let client = common::client();
    let search = client.search_paper("genome wide association", 3, None).await?;
    let ids: Vec<&str> = search.data.iter().map(|p| p.paper_id.as_str()).collect();

    throttle().await;
    let batch = client.get_papers_batch(&ids, None).await?;
    assert_eq!(batch.len(), 3);
    for entry in &batch {
        assert!(entry.is_some(), "batch entry should be found");
    }

    Ok(())
}

// ===========================================================================
// Citations / References
// ===========================================================================

#[tokio::test]
#[ignore]
#[serial]
async fn get_citations() -> TestResult {
    throttle().await;
    let client = common::client();
    let resp = client
        .get_citations("ARXIV:1706.03762", 5, 0, None)
        .await?;

    assert!(!resp.data.is_empty(), "should have citations");
    let c = &resp.data[0];
    assert!(
        !c.citing_paper.paper_id.is_empty(),
        "citingPaper should have paperId"
    );

    Ok(())
}

#[tokio::test]
#[ignore]
#[serial]
async fn get_references() -> TestResult {
    throttle().await;
    let client = common::client();
    let resp = client
        .get_references("ARXIV:1706.03762", 5, 0, None)
        .await?;

    assert!(!resp.data.is_empty(), "should have references");
    let r = &resp.data[0];
    assert!(
        !r.cited_paper.paper_id.is_empty(),
        "citedPaper should have paperId"
    );

    Ok(())
}

#[tokio::test]
#[ignore]
#[serial]
async fn citation_intents_present() -> TestResult {
    throttle().await;
    let client = common::client();
    // Use a large, well-studied paper for richer citation metadata.
    let resp = client
        .get_citations("ARXIV:1706.03762", 100, 0, None)
        .await?;

    // Intents are a best-effort S2 feature and may be empty for many
    // citations. We verify the field deserialises correctly but don't
    // assert non-empty (API behavior varies).
    let with_intents = resp
        .data
        .iter()
        .filter(|c| c.intents.as_ref().map(|v| !v.is_empty()).unwrap_or(false))
        .count();
    eprintln!("  {}/{} citations have intents", with_intents, resp.data.len());
    // At minimum, the field should be present (even if empty list).
    let has_field = resp
        .data
        .iter()
        .all(|c| c.intents.is_some());
    assert!(
        has_field,
        "intents field should be deserialised (present) on all citations"
    );

    Ok(())
}

// ===========================================================================
// Author search
// ===========================================================================

#[tokio::test]
#[ignore]
#[serial]
async fn search_authors() -> TestResult {
    throttle().await;
    let client = common::client();
    let resp = client.search_authors("Andrew Ng", 5, 0, None).await?;

    assert!(resp.total > 0, "expected author results");
    assert!(!resp.data.is_empty());

    let a = &resp.data[0];
    assert!(!a.author_id.is_empty());
    assert!(a.name.is_some());

    Ok(())
}

#[tokio::test]
#[ignore]
#[serial]
async fn get_author_detail() -> TestResult {
    throttle().await;
    let client = common::client();
    let search = client.search_authors("Oren Etzioni", 1, 0, None).await?;
    if search.data.is_empty() {
        return Ok(());
    }
    let author_id = &search.data[0].author_id;

    throttle().await;
    let author = client.get_author(author_id, None).await?;
    assert_eq!(author.author_id, *author_id);
    assert!(author.name.is_some());

    Ok(())
}

#[tokio::test]
#[ignore]
#[serial]
async fn get_author_papers() -> TestResult {
    throttle().await;
    let client = common::client();
    let search = client.search_authors("Andrew Ng", 1, 0, None).await?;
    if search.data.is_empty() {
        return Ok(());
    }
    let author_id = &search.data[0].author_id;

    throttle().await;
    let resp = client.get_author_papers(author_id, 5, 0, None).await?;
    assert!(!resp.data.is_empty(), "author should have papers");
    for p in &resp.data {
        assert!(!p.paper_id.is_empty());
    }

    Ok(())
}

// ===========================================================================
// Recommendations
// ===========================================================================

#[tokio::test]
#[ignore]
#[serial]
async fn recommendations_for_paper() -> TestResult {
    throttle().await;
    let client = common::client();
    // First search for a well-known paper to get its S2 SHA.
    let search = client.search_paper("attention is all you need", 1, None).await?;
    if search.data.is_empty() {
        return Ok(());
    }
    let paper_id = &search.data[0].paper_id;

    throttle().await;
    let resp = client.recommendations(paper_id, 5, None).await?;

    // Recommendations may be empty for some papers — the important thing
    // is that the API call succeeded and the response parsed correctly.
    if resp.recommended_papers.is_empty() {
        eprintln!(
            "  NOTE: no recommendations returned for paper {} ({})",
            paper_id,
            search.data[0].title.as_deref().unwrap_or("?")
        );
        return Ok(());
    }

    let p = &resp.recommended_papers[0];
    assert!(!p.paper_id.is_empty());
    assert!(p.title.is_some());

    Ok(())
}

// ===========================================================================
// Bulk search
// ===========================================================================

#[tokio::test]
#[ignore]
#[serial]
async fn bulk_search_basic() -> TestResult {
    throttle().await;
    let client = common::client();
    let resp = client
        .search_paper_bulk("gene editing CRISPR", None, None, None)
        .await?;

    assert!(resp.total > 0, "bulk search should return results");
    assert!(!resp.data.is_empty());

    Ok(())
}

#[tokio::test]
#[ignore]
#[serial]
async fn bulk_search_boolean_query() -> TestResult {
    throttle().await;
    let client = common::client();
    let resp = client
        .search_paper_bulk("transformer | \"attention mechanism\"", None, None, None)
        .await?;

    assert!(resp.total > 0);

    Ok(())
}

// ===========================================================================
// Convert layer (Paper → Article)
// ===========================================================================

#[tokio::test]
#[ignore]
#[serial]
async fn convert_search_results_to_articles() -> TestResult {
    throttle().await;
    let client = common::client();
    let resp = client.search_paper("GWAS schizophrenia", 5, None).await?;
    if resp.data.is_empty() {
        return Ok(());
    }

    let articles: Vec<_> = resp.data.iter().map(paper_to_article).collect();
    for a in &articles {
        assert_eq!(a.source, ArticleSource::SemanticScholar);
        assert!(!a.title.is_empty(), "title should not be empty");
        assert!(
            a.identifier(IdKind::S2).is_some(),
            "S2 identifier should be present"
        );
        assert!(
            !a.keywords.iter().any(|k| k.starts_with("TLDR")),
            "keywords should not contain TLDR"
        );
    }

    Ok(())
}

#[tokio::test]
#[ignore]
#[serial]
async fn convert_paper_with_doi_has_both_ids() -> TestResult {
    throttle().await;
    let client = common::client();
    let paper = client.get_paper("ARXIV:1706.03762", None).await?;
    let article = paper_to_article(&paper);

    assert!(
        article.identifier(IdKind::S2).is_some(),
        "S2 ID must be present"
    );
    assert!(
        article.identifier(IdKind::Arxiv).is_some()
            || article.doi().is_some(),
        "at least one external ID (ArXiv or DOI) should be present"
    );

    Ok(())
}
