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
use bib_types::{IdKind, Identifier};

use crate::query::LiteratureGateway;

// ---------------------------------------------------------------------------
// lit_search
// ---------------------------------------------------------------------------

#[tool(
    name = "lit_search",
    description = "Search for academic literature. By default searches ALL registered sources \
                  concurrently (PubMed, arXiv, bioRxiv, OpenAlex, Crossref, Semantic Scholar). \
                  Pass `sources` to restrict to specific sources only — those will be searched \
                  concurrently and the rest skipped entirely. Every requested source appears in \
                  `source_summary`; sources with status \"error\" did not complete and must not be \
                  interpreted as zero matches. \
                  \
                  \
        **Sources**: \"pubmed\" (biomedical), \"arxiv\" (physics/CS/math preprints), \
        \"biorxiv\" (biology/medicine preprints — keyword search not supported, fetch only), \
        \"openalex\" (270M+ works, all disciplines), \"crossref\" (DOI-registered works; \
        loose OR/recall search, strict AND is unsupported), \"semantic_scholar\" (AI-powered \
        academic search). \
        \
        **Keywords** (CRITICAL — read carefully): each array element is ONE \
        distinct search term or phrase — NOT a full sentence. Split your query \
        into separate elements so the search engine can apply `keywords_op` \
        (AND/OR) between them. \
        \
        ✅ Correct: keywords=[\"gut microbiome\", \"cardiovascular disease\"] \
        ✅ Correct: keywords=[\"CRISPR\", \"off-target\"] \
        ❌ WRONG:  keywords=[\"gut microbiome cardiovascular disease prediction\"]  \
                     (entire sentence as one element — search will miss everything) \
        \
        **Examples**: \
        • keywords=[\"CRISPR\", \"off-target\"], keywords_op=\"AND\", sources=[\"pubmed\", \"arxiv\"] \
        • keywords=[\"transformer\"], sources=[\"arxiv\"] — only arXiv \
        • keywords=[\"GWAS\"], sources=[\"pubmed\", \"openalex\"] — PubMed + OpenAlex concurrently"
)]
pub struct LitSearchInput {
    #[desc = "Search terms — each element is ONE distinct term/phrase. Split your \
             query into separate elements. \
             ✅ [\"gut microbiome\", \"cardiovascular\"] ❌ [\"gut microbiome cardiovascular\"]"]
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

    #[desc = "Sources to search (concurrently). Options: \"pubmed\", \"arxiv\", \"biorxiv\", \
             \"openalex\", \"crossref\", \"semantic_scholar\". Default: all registered sources."]
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
                let mut summary = serde_json::json!({
                    "source": b.source,
                    "status": if b.error.is_some() { "error" } else { "ok" },
                    "returned": b.articles.len(),
                });
                if let Some(error) = &b.error {
                    summary["error"] = serde_json::Value::String(error.clone());
                } else {
                    summary["total_available"] = serde_json::Value::from(b.total);
                }
                summary
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
            "sources_searched": batches.iter().map(|b| b.source.clone()).collect::<Vec<_>>(),
            "source_summary": source_summary,
            "failed_sources": batches.iter().filter(|b| b.error.is_some()).map(|b| b.source.clone()).collect::<Vec<_>>(),
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
    description = "Fetch a single article by a typed identifier from a **specific** source. \
                  You MUST specify `id_type`, `id`, AND `source`. \
                  \
        **id_type** — one of: \
        • \"doi\" — Digital Object Identifier (e.g. \"10.1038/s41586-023-06236-2\") \
        • \"pmid\" — PubMed ID (e.g. \"30124452\") \
        • \"arxiv\" — arXiv preprint ID (e.g. \"2401.00001\") \
        • \"openalex\" — OpenAlex work ID (e.g. \"W2741809807\") \
        • \"s2\" — Semantic Scholar paper ID (e.g. \"CorpusId:12345\" or 40-char SHA) \
        • \"biorxiv\" — bioRxiv/medRxiv DOI (e.g. \"10.1101/2024.01.01.574000\") \
        \
        **source** — which source to query (must be compatible with id_type): \
        • \"pubmed\" — supports pmid, doi \
        • \"arxiv\" — supports arxiv \
        • \"biorxiv\" — supports doi, biorxiv \
        • \"openalex\" — supports openalex, doi, pmid \
        • \"crossref\" — supports doi \
        • \"semantic_scholar\" — supports s2, doi, arxiv, pmid \
        \
        Returns full metadata including abstract."
)]
pub struct LitFetchInput {
    #[desc = "Identifier type: \"doi\", \"pmid\", \"arxiv\", \"openalex\", \"s2\", or \"biorxiv\""]
    pub id_type: String,

    #[desc = "The identifier value (e.g. \"10.1038/...\", \"30124452\", \"W2741809807\")"]
    pub id: String,

    #[desc = "REQUIRED — the source to query: \"pubmed\", \"arxiv\", \"biorxiv\", \"openalex\", \
             \"crossref\", or \"semantic_scholar\". There is no default; you must pick one."]
    pub source: String,
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
        let kind = parse_id_kind(&input.id_type).ok_or_else(|| ToolError::ExecutionFailed {
            source: format!(
                "unknown id_type '{}': expected one of doi, pmid, arxiv, openalex, s2, biorxiv",
                input.id_type
            )
            .into(),
        })?;
        let identifier = Identifier::new(kind, &input.id);

        // Source is mandatory — no default multi-source fallback.
        let result = self
            .gateway
            .fetch_from(&input.source, &identifier)
            .await
            .map_err(|e| ToolError::ExecutionFailed { source: e.into() })?
            .map(|article| (input.source.clone(), article));

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
                "id_type": input.id_type,
                "id": input.id,
                "message": "No article found with this identifier in any compatible source."
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
    // Defensive: if the agent passed a single element containing spaces, split it
    // into separate keywords so AND/OR actually applies between concepts.
    let keywords = input.keywords.as_ref().map(|kw| {
        if kw.len() == 1 && kw[0].split_whitespace().count() > 1 {
            kw[0].split_whitespace().map(|s| s.to_owned()).collect()
        } else {
            kw.clone()
        }
    });

    StructuredSearch {
        keywords,
        keywords_op: input
            .keywords_op
            .as_deref()
            .map(|s| match s.to_uppercase().as_str() {
                "AND" => BoolOp::And,
                "NOT" => BoolOp::Not,
                _ => BoolOp::Or,
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

/// Parse a user-supplied `id_type` string into an [`IdKind`].
///
/// Accepts the lowercase labels produced by [`IdKind::as_str`] plus
/// `"biorxiv"`.
pub(crate) fn parse_id_kind(s: &str) -> Option<IdKind> {
    match s.trim().to_lowercase().as_str() {
        "doi" => Some(IdKind::Doi),
        "pmid" => Some(IdKind::Pmid),
        "pmc" => Some(IdKind::Pmc),
        "arxiv" => Some(IdKind::Arxiv),
        "biorxiv" => Some(IdKind::Biorxiv),
        "s2" => Some(IdKind::S2),
        "openalex" => Some(IdKind::OpenAlex),
        _ => None,
    }
}

/// Return the first non-DOI, non-PMID identifier value for display.
fn first_id(article: &bib_types::Article) -> Option<String> {
    article
        .identifiers
        .iter()
        .find(|i| !matches!(i.kind, bib_types::IdKind::Doi | bib_types::IdKind::Pmid))
        .map(|i| i.value.clone())
}
