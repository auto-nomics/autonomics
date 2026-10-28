//! Unified retrieval workflow tests: local + external merge, per-source
//! failure semantics, budget behavior, and the search → import loop.
//! External sources are in-process fixtures; nothing here touches the
//! network.

use std::sync::Arc;

use async_trait::async_trait;
use bib_base::query::{LiteratureGateway, LiteratureSource, SourceBatch};
use bib_base::{BibShared, unified_import, unified_search};
use bib_types::discovery::{
    DiscoveryRequest, DiscoverySort, SourceFailure, SourceFailureKind, SourcePage, SourcePlan,
};
use bib_types::unified::{UnifiedImportRequest, UnifiedImportSelection, UnifiedSearchRequest};
use bib_types::{Article, IdKind, Identifier, SearchHit, StructuredSearch};

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

struct FixtureSource {
    name: &'static str,
    articles: Vec<Article>,
    /// Non-retryable failure to return instead of a page.
    failure: Option<SourceFailure>,
    /// Never answer; used to exercise the round deadline.
    hold: bool,
}

impl FixtureSource {
    fn with_articles(name: &'static str, articles: Vec<Article>) -> Arc<Self> {
        Arc::new(Self {
            name,
            articles,
            failure: None,
            hold: false,
        })
    }
    fn failing(name: &'static str, message: &str) -> Arc<Self> {
        Arc::new(Self {
            name,
            articles: Vec::new(),
            failure: Some(SourceFailure::new(
                SourceFailureKind::RateLimited,
                message.to_string(),
                false,
            )),
            hold: false,
        })
    }
    fn holding(name: &'static str) -> Arc<Self> {
        Arc::new(Self {
            name,
            articles: Vec::new(),
            failure: None,
            hold: true,
        })
    }
}

#[async_trait]
impl LiteratureSource for FixtureSource {
    fn name(&self) -> &'static str {
        self.name
    }
    fn plan_discovery(&self, request: &DiscoveryRequest) -> Result<SourcePlan, SourceFailure> {
        Ok(SourcePlan {
            source: self.name.to_string(),
            page_size: request.page_size,
            parameters: Default::default(),
            fingerprint: "fixture".to_string(),
            notices: Vec::new(),
        })
    }
    async fn discovery_page(
        &self,
        _plan: &SourcePlan,
        _cursor: Option<&str>,
        page_size: u16,
    ) -> Result<SourcePage, SourceFailure> {
        if let Some(failure) = &self.failure {
            return Err(failure.clone());
        }
        if self.hold {
            std::future::pending().await
        }
        Ok(SourcePage {
            source: self.name.to_string(),
            articles: self
                .articles
                .iter()
                .take(usize::from(page_size))
                .cloned()
                .collect(),
            total: Some(self.articles.len() as u64),
            next_cursor: None,
            executed_parameters: Default::default(),
            query_translation: None,
            notices: Vec::new(),
        })
    }
    async fn search(
        &self,
        _query: &StructuredSearch,
        _limit: usize,
    ) -> bib_base::Result<SourceBatch> {
        unreachable!("fixtures only serve discovery pages")
    }
    async fn fetch(&self, _id: &Identifier) -> bib_base::Result<Option<Article>> {
        unreachable!("fixtures only serve discovery pages")
    }
}

fn gateway(sources: Vec<Arc<dyn LiteratureSource>>) -> Arc<LiteratureGateway> {
    let mut gateway = LiteratureGateway::new();
    for source in sources {
        gateway.add_source(source);
    }
    Arc::new(gateway)
}

fn article(id: &str, title: &str, doi: Option<&str>, year: Option<u16>) -> Article {
    let mut article = Article::new(id, title);
    article.year = year;
    if let Some(doi) = doi {
        article.identifiers.push(Identifier::doi(doi));
    }
    article
}

fn request(query: &str, sources: &[&str]) -> UnifiedSearchRequest {
    UnifiedSearchRequest {
        query: query.to_string(),
        sources: sources.iter().map(|name| (*name).to_string()).collect(),
        source_timeout_ms: 2_000,
        deadline_ms: 2_000,
        ..Default::default()
    }
}

async fn shared() -> Arc<BibShared> {
    Arc::new(BibShared::open_in_memory().await.expect("in-memory bib"))
}

// ---------------------------------------------------------------------------
// Local-only search
// ---------------------------------------------------------------------------

#[tokio::test]
async fn local_only_search_ranks_and_flags_library() {
    let shared = shared().await;
    shared
        .bib
        .upsert_article(&article(
            "alpha",
            "CRISPR base editing review",
            None,
            Some(2023),
        ))
        .await
        .unwrap();
    shared
        .bib
        .upsert_article(&article(
            "beta",
            "CRISPR base editing in plants",
            None,
            Some(2024),
        ))
        .await
        .unwrap();
    shared
        .bib
        .upsert_article(&article("gamma", "Unrelated proteomics work", None, None))
        .await
        .unwrap();

    let response = unified_search(
        &shared.bib,
        &shared.discovery_sessions,
        &gateway(vec![]),
        request("CRISPR base editing", &["local"]),
    )
    .await
    .unwrap();

    assert_eq!(response.session_id, None);
    assert!(response.completed);
    assert_eq!(response.obtained_candidates, 2);
    assert_eq!(response.candidates.len(), 2);
    for candidate in &response.candidates {
        assert!(candidate.in_library, "every local hit is in the library");
        assert!(
            candidate
                .library_article_id
                .as_deref()
                .is_some_and(|id| id == "alpha" || id == "beta")
        );
        assert!(candidate.import_candidate_id.is_none());
        assert!(
            candidate
                .snippets
                .first()
                .is_some_and(|snippet| !snippet.snippet.is_empty()),
            "local candidates carry snippets"
        );
    }
    let status = response
        .sources
        .iter()
        .find(|status| status.local)
        .expect("local source status");
    assert_eq!(status.total, Some(2));
    assert_eq!(status.delivered_records, 2);
    assert!(status.failure.is_none());
}

// ---------------------------------------------------------------------------
// Local + external merge on strong identifiers
// ---------------------------------------------------------------------------

#[tokio::test]
async fn local_and_external_merge_on_doi() {
    let shared = shared().await;
    shared
        .bib
        .upsert_article(&article(
            "saved",
            "Mendelian randomization of lipid traits",
            Some("10.9999/merged"),
            Some(2022),
        ))
        .await
        .unwrap();
    let external = FixtureSource::with_articles(
        "fakeext",
        vec![
            article(
                "ext-1",
                "Mendelian randomization of lipid traits",
                Some("10.9999/merged"),
                Some(2022),
            ),
            article(
                "ext-2",
                "An unrelated external paper",
                Some("10.9999/other"),
                Some(2021),
            ),
        ],
    );

    let response = unified_search(
        &shared.bib,
        &shared.discovery_sessions,
        &gateway(vec![external]),
        request("Mendelian randomization", &["local", "fakeext"]),
    )
    .await
    .unwrap();

    // Same DOI merges into one candidate; the unrelated one stays separate.
    assert_eq!(response.obtained_candidates, 2);
    let merged = response
        .candidates
        .iter()
        .find(|candidate| candidate.in_library)
        .expect("merged candidate flagged in_library");
    let sources: Vec<&str> = merged
        .candidate
        .records
        .iter()
        .map(|record| record.source.as_str())
        .collect();
    assert_eq!(sources, vec!["local", "fakeext"]);
    // Local record first: library metadata is authoritative for the merge.
    assert_eq!(merged.library_article_id.as_deref(), Some("saved"));
    // Import addressing points at the session candidate that contributed
    // the external record.
    assert!(merged.import_candidate_id.is_some());
    assert!(merged.import_fingerprint.is_some());

    let external_only = response
        .candidates
        .iter()
        .find(|candidate| !candidate.in_library)
        .expect("external-only candidate");
    assert!(external_only.import_candidate_id.is_some());
    assert_eq!(
        external_only
            .candidate
            .records
            .iter()
            .map(|record| record.source.as_str())
            .collect::<Vec<_>>(),
        vec!["fakeext"]
    );
    assert!(response.session_id.is_some());
    assert!(response.completed);
}

// ---------------------------------------------------------------------------
// Failure semantics
// ---------------------------------------------------------------------------

#[tokio::test]
async fn external_failure_degrades_to_local() {
    let shared = shared().await;
    shared
        .bib
        .upsert_article(&article(
            "saved",
            "Local single-cell atlas",
            None,
            Some(2024),
        ))
        .await
        .unwrap();
    let failing = FixtureSource::failing("downsource", "rate limited by fixture");

    let response = unified_search(
        &shared.bib,
        &shared.discovery_sessions,
        &gateway(vec![failing]),
        request("single-cell", &["local", "downsource"]),
    )
    .await
    .unwrap();

    // The failed source is terminal, not silently zero: failure is visible,
    // local results still flow.
    assert!(response.completed);
    assert_eq!(response.candidates.len(), 1);
    let failed = response
        .sources
        .iter()
        .find(|status| status.source == "downsource")
        .expect("failed source status");
    assert!(
        failed
            .failure
            .as_ref()
            .is_some_and(|failure| failure.message.contains("rate limited"))
    );
}

#[tokio::test]
async fn all_sources_failed_is_an_error() {
    let shared = shared().await;
    let failing = FixtureSource::failing("onlysource", "upstream is down");

    let error = unified_search(
        &shared.bib,
        &shared.discovery_sessions,
        &gateway(vec![failing]),
        request("anything", &["onlysource"]),
    )
    .await
    .unwrap_err()
    .to_string();

    assert!(error.contains("all requested sources failed"), "{error}");
    assert!(error.contains("upstream is down"), "{error}");
}

#[tokio::test]
async fn holding_source_hits_round_deadline_and_reports_timeout() {
    let shared = shared().await;
    let holding = FixtureSource::holding("slowsource");

    let response = unified_search(
        &shared.bib,
        &shared.discovery_sessions,
        &gateway(vec![holding]),
        UnifiedSearchRequest {
            query: "topic".to_string(),
            sources: vec!["slowsource".to_string()],
            source_timeout_ms: 10_000,
            deadline_ms: 300,
            ..Default::default()
        },
    )
    .await
    .unwrap();

    let slow = response
        .sources
        .iter()
        .find(|status| status.source == "slowsource")
        .expect("slow source status");
    // Either the engine round deadline failed it or our budget cancelled it;
    // both are terminal-with-reason, never a silent zero.
    assert!(
        slow.failure.is_some() || !response.completed,
        "slow source must report a failure or the response must be incomplete: {slow:?} / completed={}",
        response.completed
    );
}

#[tokio::test]
async fn local_open_access_filter_is_explicitly_unsupported() {
    let shared = shared().await;
    let mut req = request("topic", &["local"]);
    req.filters.open_access = Some(true);

    let error = unified_search(
        &shared.bib,
        &shared.discovery_sessions,
        &gateway(vec![]),
        req,
    )
    .await
    .unwrap_err()
    .to_string();

    assert!(error.contains("all requested sources failed"), "{error}");
    assert!(error.contains("open_access"), "{error}");
}

// ---------------------------------------------------------------------------
// Input validation
// ---------------------------------------------------------------------------

#[tokio::test]
async fn query_and_expression_are_mutually_exclusive() {
    let shared = shared().await;
    let mut both = request("text", &["local"]);
    both.expression = Some(bib_types::SearchExpr::Term {
        value: "astext".to_string(),
        field: None,
    });
    let error = unified_search(
        &shared.bib,
        &shared.discovery_sessions,
        &gateway(vec![]),
        both,
    )
    .await
    .unwrap_err()
    .to_string();
    assert!(error.contains("exactly one"), "{error}");

    let neither = UnifiedSearchRequest {
        sources: vec!["local".to_string()],
        ..Default::default()
    };
    let error = unified_search(
        &shared.bib,
        &shared.discovery_sessions,
        &gateway(vec![]),
        neither,
    )
    .await
    .unwrap_err()
    .to_string();
    assert!(error.contains("exactly one"), "{error}");
}

// ---------------------------------------------------------------------------
// Harvest paging and ordering
// ---------------------------------------------------------------------------

#[tokio::test]
async fn harvest_pages_across_session_result_pages() {
    let shared = shared().await;
    let bulk = |source: &'static str, prefix: &str| {
        FixtureSource::with_articles(
            source,
            (0..100)
                .map(|index| {
                    article(
                        &format!("{prefix}-{index}"),
                        &format!("Bulk paper {prefix} {index}"),
                        Some(&format!("10.9999/{prefix}-{index}")),
                        Some(2020 + (index % 5) as u16),
                    )
                })
                .collect(),
        )
    };

    let response = unified_search(
        &shared.bib,
        &shared.discovery_sessions,
        &gateway(vec![bulk("bulka", "a"), bulk("bulkb", "b")]),
        UnifiedSearchRequest {
            query: "bulk".to_string(),
            sources: vec!["bulka".to_string(), "bulkb".to_string()],
            limit: 100,
            source_timeout_ms: 2_000,
            deadline_ms: 2_000,
            ..Default::default()
        },
    )
    .await
    .unwrap();

    // 200 merged candidates obtained; the response is capped at the limit.
    assert_eq!(response.obtained_candidates, 200);
    assert_eq!(response.candidates.len(), 100);
}

#[tokio::test]
async fn newest_sort_orders_merged_candidates_by_year() {
    let shared = shared().await;
    shared
        .bib
        .upsert_article(&article("old", "Quantum dots imaging", None, Some(2015)))
        .await
        .unwrap();
    let external = FixtureSource::with_articles(
        "fakeext",
        vec![
            article(
                "mid",
                "Quantum dots sensing",
                Some("10.9999/mid"),
                Some(2019),
            ),
            article(
                "new",
                "Quantum dots therapy",
                Some("10.9999/new"),
                Some(2025),
            ),
        ],
    );

    let mut req = request("quantum dots", &["local", "fakeext"]);
    req.sort = DiscoverySort::Newest;
    let response = unified_search(
        &shared.bib,
        &shared.discovery_sessions,
        &gateway(vec![external]),
        req,
    )
    .await
    .unwrap();

    let years: Vec<Option<u16>> = response
        .candidates
        .iter()
        .map(|candidate| candidate.candidate.records[0].article.year)
        .collect();
    let mut sorted = years.clone();
    sorted.sort_by_key(|year| std::cmp::Reverse(*year));
    assert_eq!(years, sorted, "candidates run newest first: {years:?}");
    assert_eq!(years.first(), Some(&Some(2025)));
}

// ---------------------------------------------------------------------------
// Search → import loop
// ---------------------------------------------------------------------------

#[tokio::test]
async fn import_preview_and_idempotent_import() {
    let shared = shared().await;
    shared
        .bib
        .upsert_article(&article(
            "saved",
            "Proteomics of mitochondrial dynamics",
            Some("10.9999/known"),
            Some(2023),
        ))
        .await
        .unwrap();
    let external = FixtureSource::with_articles(
        "fakeext",
        vec![
            article(
                "ext-known",
                "Proteomics of mitochondrial dynamics",
                Some("10.9999/known"),
                Some(2023),
            ),
            article(
                "ext-new",
                "Phosphoproteomics workflow advances",
                Some("10.9999/new"),
                Some(2024),
            ),
        ],
    );

    let search = unified_search(
        &shared.bib,
        &shared.discovery_sessions,
        &gateway(vec![external]),
        request("proteomics", &["local", "fakeext"]),
    )
    .await
    .unwrap();
    let session_id = search.session_id.clone().expect("external session");

    let selections: Vec<UnifiedImportSelection> = search
        .candidates
        .iter()
        .filter_map(|candidate| {
            let candidate_id = candidate.import_candidate_id.as_deref()?;
            Some(UnifiedImportSelection {
                candidate_id: candidate_id.to_string(),
                expected_fingerprint: candidate.import_fingerprint.clone()?,
                record_index: 0,
            })
        })
        .collect();
    assert_eq!(selections.len(), 2, "both candidates are importable");

    // Preview: the already-saved candidate matches the library.
    let preview = unified_import(
        &shared,
        UnifiedImportRequest {
            session_id: session_id.clone(),
            selections: selections.clone(),
            dry_run: true,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let preview = preview.preview.expect("preview payload");
    let matched = preview
        .items
        .iter()
        .filter(|item| !item.existing.is_empty())
        .count();
    assert_eq!(matched, 1, "the DOI already in the library matches");

    // Import: one already present, one imported.
    let imported = unified_import(
        &shared,
        UnifiedImportRequest {
            session_id: session_id.clone(),
            selections: selections.clone(),
            dry_run: false,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(imported.imported, 1);
    assert_eq!(imported.failed, 0);
    let statuses: Vec<&str> = imported
        .outcomes
        .iter()
        .map(|outcome| match outcome.status {
            bib_types::discovery::DiscoveryImportStatus::Imported => "imported",
            bib_types::discovery::DiscoveryImportStatus::AlreadyPresent => "already",
            bib_types::discovery::DiscoveryImportStatus::Failed => "failed",
        })
        .collect();
    assert!(statuses.contains(&"imported"));
    assert!(statuses.contains(&"already"));

    // Re-import is idempotent.
    let again = unified_import(
        &shared,
        UnifiedImportRequest {
            session_id,
            selections,
            dry_run: false,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(again.imported, 0);
    assert_eq!(again.failed, 0);

    // The new article really is in the library and searchable.
    let page = shared
        .bib
        .search_v2(bib_types::SearchRequestV2 {
            query: "Phosphoproteomics".to_string(),
            include_articles: true,
            ..Default::default()
        })
        .await
        .unwrap();
    assert!(
        page.hits
            .iter()
            .any(|hit: &SearchHit| hit.title.contains("Phosphoproteomics"))
    );
}

#[tokio::test]
async fn stale_fingerprint_rejects_import() {
    let shared = shared().await;
    let external = FixtureSource::with_articles(
        "fakeext",
        vec![article(
            "ext-1",
            "Retracted analysis",
            Some("10.9999/stale"),
            Some(2020),
        )],
    );
    let search = unified_search(
        &shared.bib,
        &shared.discovery_sessions,
        &gateway(vec![external]),
        request("retracted", &["fakeext"]),
    )
    .await
    .unwrap();
    let candidate = &search.candidates[0];

    let error = unified_import(
        &shared,
        UnifiedImportRequest {
            session_id: search.session_id.clone().unwrap(),
            selections: vec![UnifiedImportSelection {
                candidate_id: candidate.import_candidate_id.clone().unwrap(),
                expected_fingerprint: "not-the-real-fingerprint".to_string(),
                record_index: 0,
            }],
            dry_run: false,
            ..Default::default()
        },
    )
    .await
    .unwrap_err()
    .to_string();
    assert!(
        error.contains("fingerprint") || error.contains("candidate metadata changed"),
        "{error}"
    );
}
