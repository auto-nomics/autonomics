//! Unified retrieval agent tools: `literature_search` (one query across
//! the local library and external sources, merged and ranked) and
//! `literature_import` (bring selected candidates into the library with
//! the discovery import path).
//!
//! These complement — not replace — the DAG evidence channel: a single
//! retrieval decision uses the tools; multi-step pipelines (search →
//! merge → export → save) keep `source_literature` / `evidence_merge` /
//! `evidence_export` / `bib_save`.

use std::sync::Arc;

use agentik_core::tools::{ToolError, ToolFunction, ToolRegistration};
use agentik_proc::tool;
use agentik_sdk::types::ToolResult as AgentToolResult;
use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use bib_types::discovery::{DiscoveryFilters, DiscoverySort};
use bib_types::unified::{
    UnifiedCandidate, UnifiedImportRequest, UnifiedImportSelection, UnifiedSearchRequest,
};
use bib_types::{Article, SearchMode};

use crate::shared::BibShared;
use crate::unified_search;

// ===========================================================================
// literature_search
// ===========================================================================

#[tool(
    name = "literature_search",
    description = "Search for literature in ONE call across the local library AND external \
                  databases (PubMed, OpenAlex, arXiv, Crossref, Semantic Scholar), with \
                  cross-source deduplication: the same paper found locally and externally \
                  appears once, flagged `in_library`. \
                  \
                  Query syntax (local engine; external sources translate it): bare terms \
                  AND together, `OR`/`NOT`, quoted `\"exact phrases\"`, field filters \
                  `title:… author:… year:2020..2026 tag:… journal:… abstract:…`, \
                  parentheses for grouping. \
                  \
                  Sources: `local` (your saved library, full-text and annotations), plus \
                  `pubmed`, `openalex`, `arxiv`, `crossref`, `semantic_scholar`. Default \
                  `[local, pubmed, openalex]`. Use `[\"local\"]` for library-only. \
                  \
                  Returns merged candidates (title, authors, year, DOI, snippet, \
                  provenance) plus per-source status. Slow sources are cancelled at \
                  `deadline_ms`; partial results and failures are reported, never shown \
                  as zero results. \
                  \
                  To save external candidates into the library, follow up with \
                  `literature_import` using this response's `session_id` and each \
                  candidate's `import_candidate_id` + `import_fingerprint` \
                  (absent for candidates already fully in the library). \
                  \
                  For library management (collections, notes, export) use the `bib_*` \
                  tools; for full-text reading use `bib_get_article`. \
                  \
                  **Examples**: \
                  • query=\"APOE AND lipid metabolism\" — default local+pubmed+openalex \
                  • query=\"title:CRISPR base editing\", sources=[\"local\"] — library only \
                  • query=\"single cell RNA-seq\", year_from=2022, sort=\"newest\""
)]
pub struct LiteratureSearchInput {
    #[desc = "Query (local syntax: terms, OR/NOT, \"phrases\", field:value, year:A..B, parentheses)"]
    pub query: String,
    #[desc = "Sources to search: `local` and/or `pubmed`, `openalex`, `arxiv`, `crossref`, `semantic_scholar`. Default [local, pubmed, openalex]"]
    pub sources: Option<Vec<String>>,
    #[desc = "Publication year lower bound"]
    pub year_from: Option<u16>,
    #[desc = "Publication year upper bound"]
    pub year_to: Option<u16>,
    #[desc = "Publication types to keep (e.g. [\"Journal Article\", \"Review\"])"]
    pub publication_types: Option<Vec<String>>,
    #[desc = "Only open-access results (external sources only; the local source reports unsupported)"]
    pub open_access: Option<bool>,
    #[desc = "Merged ordering: \"relevance\" (default), \"newest\", \"oldest\", \"citation_count\" (per-source native order)"]
    pub sort: Option<String>,
    #[desc = "Local recall mode: \"lexical\" (default, exact semantics) or \"hybrid\" (adds semantic recall; needs the semantic service)"]
    pub mode: Option<String>,
    #[desc = "Topic text for local hybrid recall, independent of the query (mode=\"hybrid\" only)"]
    pub semantic_query: Option<String>,
    #[desc = "Maximum merged candidates returned, 1–100 (default 25)"]
    pub limit: Option<u16>,
    #[desc = "Budget per external source including retries, ms (default 20000)"]
    pub source_timeout_ms: Option<u64>,
    #[desc = "Overall wall-clock budget, ms (default 30000); sources still running are cancelled and partial results kept"]
    pub deadline_ms: Option<u64>,
}

pub struct LiteratureSearchTool {
    pub shared: Arc<BibShared>,
}

fn parse_sort(value: Option<&str>) -> Result<DiscoverySort, ToolError> {
    match value.map(str::trim).map(str::to_lowercase).as_deref() {
        None | Some("") | Some("relevance") => Ok(DiscoverySort::Relevance),
        Some("newest") => Ok(DiscoverySort::Newest),
        Some("oldest") => Ok(DiscoverySort::Oldest),
        Some("citation_count" | "citations") => Ok(DiscoverySort::CitationCount),
        Some(other) => Err(ToolError::ExecutionFailed {
            source: format!(
                "unknown sort `{other}`: use relevance, newest, oldest, or citation_count"
            )
            .into(),
        }),
    }
}

fn parse_mode(value: Option<&str>) -> Result<SearchMode, ToolError> {
    match value.map(str::trim).map(str::to_lowercase).as_deref() {
        None | Some("") | Some("lexical") => Ok(SearchMode::Lexical),
        Some("hybrid") => Ok(SearchMode::Hybrid),
        Some(other) => Err(ToolError::ExecutionFailed {
            source: format!("unknown mode `{other}`: use lexical or hybrid").into(),
        }),
    }
}

/// Compact agent-facing rendering of a candidate. The full discovery
/// contract stays in the session (`literature_import` works against the
/// session, not this rendering); only what a retrieval decision needs is
/// echoed back.
fn compact_candidate(item: &UnifiedCandidate) -> serde_json::Value {
    let article = &item.candidate.article;
    let authors: Vec<String> = article
        .authors
        .iter()
        .take(3)
        .map(|author| {
            format!(
                "{}{}",
                author.last_name,
                author
                    .fore_name
                    .as_deref()
                    .map(|given| format!(", {given}"))
                    .unwrap_or_default()
            )
        })
        .collect();
    let identifiers: serde_json::Map<String, serde_json::Value> = article
        .identifiers
        .iter()
        .map(|id| (id.kind.as_str().to_string(), serde_json::json!(id.value)))
        .collect();
    let snippet = item
        .snippets
        .first()
        .map(|snippet| snippet.snippet.clone())
        .or_else(|| {
            article
                .abstract_text
                .as_ref()
                .map(|text| truncate(text, 240))
        });
    serde_json::json!({
        "id": item.candidate.id,
        "import_candidate_id": item.import_candidate_id,
        "import_fingerprint": item.import_fingerprint,
        "title": article.title,
        "authors": authors,
        "year": article.year,
        "journal": article.journal,
        "identifiers": identifiers,
        "in_library": item.in_library,
        "library_article_id": item.library_article_id,
        "sources": item.candidate.records.iter()
            .map(|record| record.source.as_str())
            .collect::<Vec<_>>(),
        "rrf_score": item.candidate.rrf_score,
        "snippet": snippet,
    })
}

fn truncate(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let mut cut: String = text.chars().take(max_chars).collect();
    cut.push('…');
    cut
}

#[async_trait]
impl ToolFunction for LiteratureSearchTool {
    type Input = LiteratureSearchInput;

    async fn run(&self, input: Self::Input) -> Result<AgentToolResult, ToolError> {
        if input.query.trim().is_empty() {
            return Err(ToolError::ExecutionFailed {
                source: "query must not be empty".into(),
            });
        }
        let request = UnifiedSearchRequest {
            query: input.query,
            expression: None,
            sources: input.sources.unwrap_or_default(),
            filters: DiscoveryFilters {
                year_from: input.year_from,
                year_to: input.year_to,
                publication_types: input.publication_types.unwrap_or_default(),
                open_access: input.open_access,
            },
            sort: parse_sort(input.sort.as_deref())?,
            mode: parse_mode(input.mode.as_deref())?,
            semantic_query: input.semantic_query,
            limit: input.limit.unwrap_or(25),
            source_timeout_ms: input.source_timeout_ms.unwrap_or(20_000),
            deadline_ms: input.deadline_ms.unwrap_or(30_000),
        };
        let response = self.shared.unified_search(request).await.map_err(|error| {
            ToolError::ExecutionFailed {
                source: error.to_string().into(),
            }
        })?;
        let candidates: Vec<serde_json::Value> =
            response.candidates.iter().map(compact_candidate).collect();
        let sources: Vec<serde_json::Value> = response
            .sources
            .iter()
            .map(|status| {
                serde_json::json!({
                    "source": status.source,
                    "phase": status.phase,
                    "total": status.total,
                    "records": status.delivered_records,
                    "failure": status.failure.as_ref().map(|failure| failure.message.clone()),
                })
            })
            .collect();
        Ok(AgentToolResult::success_json(serde_json::json!({
            "session_id": response.session_id,
            "completed": response.completed,
            "obtained_candidates": response.obtained_candidates,
            "n_candidates": candidates.len(),
            "sources": sources,
            "candidates": candidates,
            "message": format!(
                "{} merged candidate(s) from {} source(s){}",
                candidates.len(),
                sources.len(),
                if response.completed { "" } else { "; deadline reached — some sources were cancelled, partial results only" },
            ),
        })))
    }
}

// ===========================================================================
// literature_import
// ===========================================================================

/// One candidate to import, echoed from a `literature_search` response.
/// `expected_fingerprint` rejects the write when the session has delivered
/// newer batches for that candidate since.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ImportCandidateRef {
    /// Candidate `id` from the search response.
    pub candidate_id: String,
    /// Candidate `fingerprint` from the search response.
    pub expected_fingerprint: String,
    /// Record index within the candidate, picking a specific source's
    /// version when a candidate carries several. Default: 0.
    #[serde(default)]
    pub record_index: Option<usize>,
}

#[tool(
    name = "literature_import",
    description = "Save candidates from a `literature_search` into the local library. \
                  Takes the `session_id` and the candidates' `import_candidate_id` + \
                  that search. Idempotent: candidates already in the library (same \
                  DOI/PMID/arXiv) are reported `already_present` — existing metadata, \
                  notes and collections are never overwritten. Optional full-text \
                  fetch (PMC open access) runs as a background job per article. \
                  \
                  Run with dry_run=true first to preview matches, or directly for \
                  the import. After importing, use `bib_get_article` for full text \
                  and `bib_add_to_collection` to organize. \
                  \
                  **Example**: session_id=\"…\", candidates=[{candidate_id:\"…\", \
                  expected_fingerprint:\"…\"}], collection_id=\"ABC123\", \
                  fetch_fulltext=true"
)]
pub struct LiteratureImportInput {
    #[desc = "Session id from the literature_search response (None for a local-only search means nothing to import)"]
    pub session_id: String,
    #[desc = "Candidates to import: {candidate_id, expected_fingerprint, record_index?} from the search response"]
    pub candidates: Vec<ImportCandidateRef>,
    #[desc = "Existing collection id to add imported articles to (create one first with bib_create_collection)"]
    pub collection_id: Option<String>,
    #[desc = "Target library id (default: the default library)"]
    pub library_id: Option<i64>,
    #[desc = "Fetch open-access full text in the background where available (default false)"]
    pub fetch_fulltext: Option<bool>,
    #[desc = "Preview matches without writing (default false)"]
    pub dry_run: Option<bool>,
}

pub struct LiteratureImportTool {
    pub shared: Arc<BibShared>,
}

#[async_trait]
impl ToolFunction for LiteratureImportTool {
    type Input = LiteratureImportInput;

    async fn run(&self, input: Self::Input) -> Result<AgentToolResult, ToolError> {
        if input.candidates.is_empty() {
            return Err(ToolError::ExecutionFailed {
                source: "candidates must list at least one candidate".into(),
            });
        }
        let selections: Vec<UnifiedImportSelection> = input
            .candidates
            .iter()
            .map(|reference| UnifiedImportSelection {
                candidate_id: reference.candidate_id.clone(),
                expected_fingerprint: reference.expected_fingerprint.clone(),
                record_index: reference.record_index.unwrap_or(0),
            })
            .collect();
        let request = UnifiedImportRequest {
            session_id: input.session_id,
            selections,
            collection_id: input.collection_id,
            library_id: input.library_id,
            fetch_fulltext: input.fetch_fulltext.unwrap_or(false),
            dry_run: input.dry_run.unwrap_or(false),
        };
        let response = self.shared.unified_import(request).await.map_err(|error| {
            ToolError::ExecutionFailed {
                source: error.to_string().into(),
            }
        })?;
        if let Some(preview) = &response.preview {
            let items: Vec<serde_json::Value> = preview
                .items
                .iter()
                .map(|match_item| {
                    serde_json::json!({
                        "candidate_id": match_item.candidate_id,
                        "already_in_library": !match_item.existing.is_empty(),
                        "existing_article_ids": match_item.existing.iter()
                            .map(|article| article.id.clone()).collect::<Vec<_>>(),
                        "related_versions_in_library": match_item.related_existing.len(),
                        "field_differences": match_item.differences.len(),
                        "error": match_item.error,
                    })
                })
                .collect();
            return Ok(AgentToolResult::success_json(serde_json::json!({
                "dry_run": true,
                "items": items,
                "message": format!(
                    "preview: {} candidate(s), {} already in library",
                    items.len(),
                    items.iter().filter(|item| item["already_in_library"].as_bool().unwrap_or(false)).count(),
                ),
            })));
        }
        let outcomes: Vec<serde_json::Value> = response
            .outcomes
            .iter()
            .map(|outcome| {
                serde_json::json!({
                    "candidate_id": outcome.candidate_id,
                    "status": outcome.status,
                    "article_id": outcome.article.as_ref().map(|article: &Article| article.id.clone()),
                    "related_article_ids": outcome.related_article_ids,
                    "collection_added": outcome.collection_added,
                    "changed": outcome.changed,
                    "error": outcome.error,
                })
            })
            .collect();
        Ok(AgentToolResult::success_json(serde_json::json!({
            "dry_run": false,
            "imported": response.imported,
            "failed": response.failed,
            "outcomes": outcomes,
            "fulltext_acquisition_job": response.acquisition.as_ref().map(|job| serde_json::json!({
                "id": job.id,
                "items": job.items.len(),
            })),
            "message": format!(
                "imported {}, already present {}, failed {}",
                response.imported,
                outcomes.iter()
                    .filter(|outcome| outcome["status"] == serde_json::json!("already_present"))
                    .count(),
                response.failed,
            ),
        })))
    }
}

// ===========================================================================
// Registration
// ===========================================================================

/// Unified retrieval tools for hosts holding the process-wide
/// [`BibShared`]. Both tools share its connection pool, discovery session
/// engine and rate-limit windows.
pub fn unified_registrations(shared: Arc<BibShared>) -> Vec<ToolRegistration> {
    vec![
        ToolRegistration::from(LiteratureSearchTool {
            shared: shared.clone(),
        }),
        ToolRegistration::from(LiteratureImportTool { shared }),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn shared() -> Arc<BibShared> {
        Arc::new(BibShared::open_in_memory().await.expect("in-memory bib"))
    }

    #[tokio::test]
    async fn registrations_expose_both_tools() {
        let tools = unified_registrations(shared().await);
        let names: Vec<_> = tools
            .iter()
            .map(|tool| tool.definition.name.clone())
            .collect();
        assert_eq!(names, ["literature_search", "literature_import"]);
        assert!(
            tools
                .iter()
                .all(|tool| !tool.definition.input_schema.properties.is_empty())
        );
    }

    #[test]
    fn sort_and_mode_parsing() {
        assert_eq!(parse_sort(None).unwrap(), DiscoverySort::Relevance);
        assert_eq!(parse_sort(Some("newest")).unwrap(), DiscoverySort::Newest);
        assert_eq!(
            parse_sort(Some("Citation_Count")).unwrap(),
            DiscoverySort::CitationCount
        );
        assert!(parse_sort(Some("bogus")).is_err());
        assert_eq!(parse_mode(None).unwrap(), SearchMode::Lexical);
        assert_eq!(parse_mode(Some("hybrid")).unwrap(), SearchMode::Hybrid);
        assert!(parse_mode(Some("fuzzy")).is_err());
    }

    #[tokio::test]
    async fn empty_query_is_rejected() {
        let tool = LiteratureSearchTool {
            shared: shared().await,
        };
        let error = tool
            .run(LiteratureSearchInput {
                query: "   ".into(),
                sources: None,
                year_from: None,
                year_to: None,
                publication_types: None,
                open_access: None,
                sort: None,
                mode: None,
                semantic_query: None,
                limit: None,
                source_timeout_ms: None,
                deadline_ms: None,
            })
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("query must not be empty"), "{error}");
    }
}
