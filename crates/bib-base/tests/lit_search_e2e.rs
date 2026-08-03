//! End-to-end integration tests for lit_search / lit_fetch.
//!
//! These tests hit the **live** PubMed / arXiv APIs and require network
//! connectivity. They are gated behind the `e2e` feature flag so CI can
//! skip them.
//!
//! Run with:  `cargo test -p bib-base --features e2e --test lit_search_e2e -- --nocapture`

#![cfg(feature = "e2e")]

use std::sync::Arc;

use bib_base::query::{LiteratureGateway, PubmedSource};
use bib_types::query::{BoolOp, StructuredSearch, YearRange};

fn pubmed_gateway() -> LiteratureGateway {
    LiteratureGateway::new().with_source(Arc::new(PubmedSource::new(Arc::new(
        eutils::EutilsClient::from_env(),
    ))))
}

#[tokio::test]
async fn pubmed_keyword_search_returns_results() {
    let gw = pubmed_gateway();

    let sq = StructuredSearch {
        keywords: Some(vec!["proteomics".into(), "cardiovascular".into()]),
        keywords_op: Some(BoolOp::And),
        ..Default::default()
    };

    let batches = gw.search(&sq, 5).await;

    assert!(!batches.is_empty(), "expected at least one source batch");
    let batch = &batches[0];
    assert_eq!(batch.source, "pubmed");
    assert!(
        batch.total > 0,
        "PubMed should have proteomics+cardiovascular papers"
    );
    assert!(!batch.articles.is_empty(), "should return actual articles");

    // Every article should mention at least one keyword.
    for a in &batch.articles {
        let blob = format!(
            "{} {}",
            a.title.to_lowercase(),
            a.abstract_text.as_deref().unwrap_or("").to_lowercase()
        );
        assert!(
            blob.contains("proteomic") || blob.contains("cardiovascular"),
            "article '{}' doesn't mention keywords",
            a.title
        );
    }

    println!(
        "✅ PubMed returned {} articles (total available: {})",
        batch.articles.len(),
        batch.total
    );
}

#[tokio::test]
async fn pubmed_year_from_open_ended_range() {
    // Regression: `year_from` without `year_to` must produce an open-ended
    // range (2023 onwards), NOT a single-year collapse (only 2023).
    let gw = pubmed_gateway();

    // Simulate what LitSearchTool.build_structured_search now does.
    let year_from = 2023u16;
    let sq = StructuredSearch {
        keywords: Some(vec!["proteomics".into(), "cardiovascular".into()]),
        keywords_op: Some(BoolOp::And),
        year_range: Some(YearRange {
            from: year_from,
            to: 3000, // open-ended upper bound
        }),
        ..Default::default()
    };

    let batches = gw.search(&sq, 10).await;
    let batch = &batches[0];

    // With an open-ended range we should get papers from 2023, 2024, 2025, 2026…
    let years: Vec<_> = batch
        .articles
        .iter()
        .filter_map(|a| a.year)
        .collect::<std::collections::HashSet<_>>()
        .into_iter()
        .collect();

    println!("Years found in results: {:?}", years);
    println!("Total available: {}", batch.total);

    // The open-ended range should match far more than a single-year filter.
    // "proteomics AND cardiovascular" from 2023+ has 1000+ papers on PubMed.
    assert!(
        batch.total > 100,
        "open-ended range should match hundreds of papers; got total={}",
        batch.total
    );

    // At least one paper from a year > 2023 proves the range is open-ended.
    let has_post_2023 = years.iter().any(|&y| y > 2023);
    assert!(
        has_post_2023,
        "expected papers from after 2023, but years were {:?}",
        years
    );

    // No papers from before 2023 should leak in.
    let has_pre_2023 = years.iter().any(|&y| y < 2023);
    assert!(
        !has_pre_2023,
        "papers from before 2023 leaked through: {:?}",
        years
    );

    println!(
        "✅ Open-ended year_from works: {} distinct years {:?}",
        years.len(),
        years
    );
}

#[tokio::test]
async fn pubmed_fetch_by_pmid() {
    let gw = pubmed_gateway();

    // PMID 37658030 — a real PubMed record.
    let result = gw.fetch("37658030").await;

    assert!(result.is_some(), "should fetch PMID 37658030");
    let (source, article) = result.unwrap();
    assert_eq!(source, "pubmed");
    assert!(!article.title.is_empty());

    // EFetch (MEDLINE) must populate the abstract — ESummary never did.
    assert!(
        article.abstract_text.is_some(),
        "abstract must be populated by EFetch, got None for '{}'",
        article.title
    );
    assert!(
        article.abstract_text.as_deref().unwrap().len() > 50,
        "abstract should be a real paragraph, not a stub"
    );

    println!("✅ Fetched: {} (abstract: {} chars)", article.title, article.abstract_text.as_deref().unwrap().len());
}

#[tokio::test]
async fn bib_save_batch_mixed_ids() {
    use bib_base::BibBase;

    // In-memory DB + gateway with all default sources.
    let bib = Arc::new(BibBase::open_in_memory().await.unwrap());
    let gateway = Arc::new(bib_base::default_gateway());

    // Build the tool directly (not via the agent framework).
    use agentik_core::tools::ToolFunction;
    use agentik_sdk::types::ToolResultContent;
    use bib_base::library_tools::{BibSaveInput, BibSaveTool};
    use europepmc::EuropePmcClient;

    let epmc = Arc::new(EuropePmcClient::new());
    let tool = BibSaveTool { bib, gateway, epmc };

    // Mix of real PMIDs — these are stable PubMed records.
    let input = BibSaveInput {
        ids: vec!["37658030".into(), "37506997".into()],
        source: None,
        fetch_fulltext: Some(false),
    };

    let result = tool.run(input).await.unwrap();
    let json = match result.content {
        ToolResultContent::Json(v) => v,
        _ => panic!("expected JSON content"),
    };

    assert_eq!(json["total"], 2, "should process both IDs");
    assert_eq!(
        json["saved"].as_u64(),
        Some(2),
        "both should be newly saved"
    );
    assert_eq!(json["cached"].as_u64(), Some(0), "none should be cached");

    let results = json["results"].as_array().unwrap();
    assert_eq!(results.len(), 2);
    for r in results {
        assert_eq!(r["saved"], true);
        assert!(r["article_id"].as_str().is_some());
        assert!(r["title"].as_str().is_some());
    }

    println!("✅ Batch save: {}", json["message"]);

    // Second call — both should be cached now.
    let input2 = BibSaveInput {
        ids: vec!["37658030".into(), "37506997".into()],
        source: None,
        fetch_fulltext: Some(false),
    };
    let result2 = tool.run(input2).await.unwrap();
    let json2 = match result2.content {
        ToolResultContent::Json(v) => v,
        _ => panic!("expected JSON content"),
    };

    assert_eq!(json2["saved"].as_u64(), Some(0), "none newly saved");
    assert_eq!(json2["cached"].as_u64(), Some(2), "both cached");

    println!("✅ Batch re-save (cached): {}", json2["message"]);
}
