//! Agent tool layer for the unified literature query gateway.
//!
//! Exposes two tools:
//! - `lit_search` — concurrent multi-source search.
//! - `lit_fetch`  — single-article fetch with auto-detected source.
//!
//! Wire into an agent via [`bib_query_registrations`].

use std::sync::Arc;

use agentik_core::tools::{ToolError, ToolFunction, ToolRegistration};
use agentik_proc::tool;
use agentik_sdk::types::ToolResult as AgentToolResult;
use async_trait::async_trait;
use bib_types::query::{BoolOp, StructuredSearch, YearRange};

use crate::query::LiteratureGateway;

// ---------------------------------------------------------------------------
// lit_search
// ---------------------------------------------------------------------------

#[tool(
    name = "lit_search",
    description = "Search for academic literature. By default searches ALL registered sources \
                  concurrently (PubMed, arXiv, bioRxiv). Pass `sources` to restrict to specific \
                  sources only — those will be searched concurrently and the rest skipped entirely. \
                  \
                  \
        **Sources**: \"pubmed\" (biomedical), \"arxiv\" (physics/CS/math preprints), \
        \"biorxiv\" (biology/medicine preprints — keyword search not supported, fetch only). \
        \
        **Keywords**: searched against title + abstract. Use `keywords_op` to control \
        whether ALL keywords must match (AND) or ANY (OR, default). \
        \
        **Examples**: \
        • keywords=[\"CRISPR\", \"off-target\"], keywords_op=\"AND\" — search all sources \
        • keywords=[\"transformer\"], sources=[\"arxiv\"] — only arXiv \
        • keywords=[\"GWAS\"], sources=[\"pubmed\", \"biorxiv\"] — PubMed + bioRxiv concurrently"
)]
pub struct LitSearchInput {
    #[desc = "Topic keywords to search in title/abstract, e.g. [\"CRISPR\", \"gene editing\"]"]
    pub keywords: Option<Vec<String>>,

    #[desc = "Join operator for keywords: \"AND\" (all must match) or \"OR\" (any matches). Default: OR"]
    pub keywords_op: Option<String>,

    #[desc = "Terms restricted to article title only"]
    pub title: Option<Vec<String>>,

    #[desc = "Author names, e.g. [\"Smith J\", \"Doe K\"]"]
    pub authors: Option<Vec<String>>,

    #[desc = "Journal names"]
    pub journal: Option<Vec<String>>,

    #[desc = "Publication types, e.g. [\"Review\", \"Clinical Trial\"]"]
    pub publication_types: Option<Vec<String>>,

    #[desc = "Publication year start (inclusive), e.g. 2020"]
    pub year_from: Option<u16>,

    #[desc = "Publication year end (inclusive), e.g. 2024"]
    pub year_to: Option<u16>,

    #[desc = "Sources to search (concurrently). Options: \"pubmed\", \"arxiv\", \"biorxiv\". \
             Default: all registered sources."]
    pub sources: Option<Vec<String>>,

    #[desc = "Max results per source (default 10)"]
    pub limit: Option<usize>,
}

pub struct LitSearchTool {
    pub gateway: Arc<LiteratureGateway>,
}

#[async_trait]
impl ToolFunction for LitSearchTool {
    type Input = LitSearchInput;

    fn timeout_seconds(&self) -> u64 {
        120
    }

    async fn run(&self, input: Self::Input) -> Result<AgentToolResult, ToolError> {
        let sq = build_structured_search(&input);

        if sq.keywords.is_none()
            && sq.title.is_none()
            && sq.authors.is_none()
            && sq.journal.is_none()
            && sq.publication_types.is_none()
            && sq.year_range.is_none()
        {
            return Err(ToolError::ExecutionFailed {
                source: "at least one search field is required".into(),
            });
        }

        let limit = input.limit.unwrap_or(10).clamp(1, 100);

        // Dispatch only to the specified sources (or all if not specified).
        let batches = self
            .gateway
            .search_named(input.sources.as_deref(), &sq, limit)
            .await;

        // Build compact result JSON for the agent.
        let total_articles: usize = batches.iter().map(|b| b.articles.len()).sum();
        let source_summary: Vec<serde_json::Value> = batches
            .iter()
            .map(|b| {
                serde_json::json!({
                    "source": b.source,
                    "total_available": b.total,
                    "returned": b.articles.len(),
                })
            })
            .collect();

        let articles_json: Vec<serde_json::Value> = batches
            .iter()
            .flat_map(|b| {
                b.articles.iter().map(|a| {
                    serde_json::json!({
                        "id": a.id,
                        "source": b.source,
                        "title": a.title,
                        "authors": a.authors.iter().map(|au| au.display_name()).collect::<Vec<_>>(),
                        "year": a.year,
                        "journal": a.journal,
                        "doi": a.doi(),
                        "pmid": a.pmid(),
                        "identifier": first_id(a),
                        "abstract": a.abstract_text.as_deref().map(|s| {
                            if s.len() > 500 { format!("{}…", &s[..500]) } else { s.to_owned() }
                        }),
                    })
                })
            })
            .collect();

        let result = serde_json::json!({
            "sources_searched": self.gateway.source_names(),
            "source_summary": source_summary,
            "total_returned": total_articles,
            "articles": articles_json,
        });

        Ok(AgentToolResult::success_json(result))
    }
}

// ---------------------------------------------------------------------------
// lit_fetch
// ---------------------------------------------------------------------------

#[tool(
    name = "lit_fetch",
    description = "Fetch a single article by ID from the appropriate source. \
                  Auto-detects the source from the ID format: \
                  • Numeric (e.g. \"30124452\") → PubMed PMID \
                  • Starts with \"10.\" → DOI (resolved via PubMed) \
                  • Contains \"/\" or dot pattern (e.g. \"2401.12345\") → arXiv \
                  \
                  Returns full metadata including abstract."
)]
pub struct LitFetchInput {
    #[desc = "Article identifier: PMID, DOI, or arXiv ID"]
    pub id: String,

    #[desc = "Force a specific source: \"pubmed\" or \"arxiv\". Default: auto-detect."]
    pub source: Option<String>,
}

pub struct LitFetchTool {
    pub gateway: Arc<LiteratureGateway>,
}

#[async_trait]
impl ToolFunction for LitFetchTool {
    type Input = LitFetchInput;

    fn timeout_seconds(&self) -> u64 {
        60
    }

    async fn run(&self, input: Self::Input) -> Result<AgentToolResult, ToolError> {
        let result = if let Some(ref source) = input.source {
            // Explicit source selection.
            let article = self
                .gateway
                .fetch_from(source, &input.id)
                .await
                .map_err(|e| ToolError::ExecutionFailed { source: e.into() })?;
            article.map(|a| (source.clone(), a))
        } else {
            // Auto-detect: try the most likely source first.
            auto_fetch(&self.gateway, &input.id).await
        };

        match result {
            Some((source, article)) => {
                let json = serde_json::json!({
                    "source": source,
                    "article": {
                        "id": article.id,
                        "title": article.title,
                        "authors": article.authors.iter().map(|a| a.display_name()).collect::<Vec<_>>(),
                        "year": article.year,
                        "month": article.month,
                        "journal": article.journal,
                        "volume": article.volume,
                        "issue": article.issue,
                        "pages": article.pages,
                        "doi": article.doi(),
                        "pmid": article.pmid(),
                        "identifiers": article.identifiers.iter().map(|i| {
                            serde_json::json!({"kind": i.kind.as_str(), "value": i.value})
                        }).collect::<Vec<_>>(),
                        "abstract": article.abstract_text,
                        "keywords": article.keywords,
                        "pub_types": article.pub_types,
                    }
                });
                Ok(AgentToolResult::success_json(json))
            }
            None => Ok(AgentToolResult::success_json(serde_json::json!({
                "found": false,
                "id": input.id,
                "message": "No article found with this ID in any configured source."
            }))),
        }
    }
}

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

/// Build [`ToolRegistration`]s for all literature query tools.
///
/// Pass a shared [`LiteratureGateway`] so every tool reuses the same
/// source connections.
pub fn bib_query_registrations(gateway: Arc<LiteratureGateway>) -> Vec<ToolRegistration> {
    use agentik_core::tools::ToolRegistration as R;
    vec![
        R::from(LitSearchTool {
            gateway: gateway.clone(),
        }),
        R::from(LitFetchTool { gateway }),
    ]
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Convert [`LitSearchInput`] fields into a [`StructuredSearch`].
fn build_structured_search(input: &LitSearchInput) -> StructuredSearch {
    StructuredSearch {
        keywords: input.keywords.clone(),
        keywords_op: input
            .keywords_op
            .as_deref()
            .and_then(|s| match s.to_uppercase().as_str() {
                "AND" => Some(BoolOp::And),
                "NOT" => Some(BoolOp::Not),
                _ => Some(BoolOp::Or),
            }),
        title: input.title.clone(),
        authors: input.authors.clone(),
        mesh: None,
        journal: input.journal.clone(),
        publication_types: input.publication_types.clone(),
        affiliation: None,
        year_range: match (input.year_from, input.year_to) {
            (Some(from), Some(to)) => Some(YearRange { from, to }),
            // Only start given → open-ended upper bound (current era).
            (Some(from), None) => Some(YearRange { from, to: 3000 }),
            // Only end given → open-ended lower bound (start of modern indexing).
            (None, Some(to)) => Some(YearRange { from: 1900, to }),
            (None, None) => None,
        },
    }
}

/// Auto-detect the source from the ID format and fetch accordingly.
///
/// Routing priority by ID format:
/// - bioRxiv/medRxiv DOI (`10.1101/...`) → bioRxiv source → all sources
/// - DOI (`10.xxx`) → PubMed → all sources
/// - Numeric (PMID) → PubMed → all sources
/// - arXiv ID pattern → arXiv → all sources
/// - Other → all sources in registration order
pub(crate) async fn auto_fetch(
    gateway: &LiteratureGateway,
    id: &str,
) -> Option<(String, bib_types::Article)> {
    let is_numeric = id.chars().all(|c| c.is_ascii_digit()) && !id.is_empty();
    let is_doi = id.starts_with("10.");
    let is_biorxiv = id.starts_with("10.1101/");

    if is_biorxiv {
        // bioRxiv/medRxiv preprint DOI — try bioRxiv source first.
        if let Some(result) = gateway.fetch_from("biorxiv", id).await.ok().flatten() {
            return Some(("biorxiv".into(), result));
        }
    }

    if is_numeric || is_doi {
        // PMID or generic DOI → try PubMed first.
        if let Some(result) = gateway.fetch_from("pubmed", id).await.ok().flatten() {
            return Some(("pubmed".into(), result));
        }
    }

    // Fall back to trying all sources in registration order.
    gateway.fetch(id).await
}

/// Return the first non-DOI, non-PMID identifier value for display.
fn first_id(article: &bib_types::Article) -> Option<String> {
    article
        .identifiers
        .iter()
        .find(|i| !matches!(i.kind, bib_types::IdKind::Doi | bib_types::IdKind::Pmid))
        .map(|i| i.value.clone())
}
