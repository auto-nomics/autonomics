//! Library management agent tools — persist, organize, and retrieve
//! articles from the local Turso-backed bibliography database.
//!
//! These tools complement the external query tools ([`lit_search`],
//! [`lit_fetch`]) by bridging "found on the internet" → "stored locally
//! and organized into collections".
//!
//! # Tool inventory
//!
//! | Tool                   | Purpose                                                |
//! |------------------------|--------------------------------------------------------|
//! | `bib_save`             | Fetch + store an article in the local library.          |
//! | `bib_delete`           | Remove articles (id- or identifier-based; dry-run safe). |
//! | `bib_create_collection`| Create a new collection.                                |
//! | `bib_add_to_collection`| Add one or more articles to a collection + roles.       |
//! | `bib_list_collection`  | List collections or articles within one.                |
//! | `bib_search_library`   | LIKE search across local library (multi-query).         |
//! | `bib_get_article`      | Batch metadata plus bounded full-text pages.             |
//! | `bib_request_fulltext` | Mark an article as needing full-text upload.            |
//! | `bib_add_note`         | Append a note / highlight / comment to an article.       |
//! | `bib_export`           | Render citation formats (BibTeX / RIS / Markdown / CSL). |

use std::sync::Arc;

use agentik_core::tools::{ToolError, ToolFunction, ToolRegistration};
use agentik_proc::tool;
use agentik_sdk::types::ToolResult as AgentToolResult;
use async_trait::async_trait;
use bib_types::{AddedBy, ArticleRole, CollectionStatus, FetchStatus, Identifier};
use europepmc::EuropePmcClient;

use crate::bib_base::BibBase;
use crate::collections::CollectionAddOutcome;
use crate::oa_fetch::try_fetch_fulltext_with;
use crate::query::LiteratureGateway;
use crate::tools::parse_id_kind;

// ===========================================================================
// bib_save — save articles to local library (direct metadata or fetch-by-id)
// ===========================================================================

/// A single typed article identifier for `bib_save`'s `ids` mode.
#[derive(schemars::JsonSchema, serde::Deserialize, serde::Serialize)]
pub struct ArticleIdInput {
    #[schemars(
        description = "Identifier type: \"doi\", \"pmid\", \"arxiv\", \"openalex\", \"s2\", or \"biorxiv\""
    )]
    pub id_type: String,
    #[schemars(
        description = "The identifier value (e.g. \"10.1038/...\", \"30124452\", \"W2741809807\")"
    )]
    pub id: String,
}

/// Complete article metadata for `bib_save`'s `articles` mode — the fast path.
///
/// Pass the full metadata you already have from `lit_search` or `lit_fetch`
/// results.  The article is stored immediately without any external
/// re-fetch, saving a network round-trip per article.
#[derive(Debug, Clone, schemars::JsonSchema, serde::Deserialize, serde::Serialize)]
pub struct ArticleInput {
    #[schemars(description = "Article title (required)")]
    pub title: String,

    #[schemars(description = "DOI, e.g. \"10.1038/s41586-023-06236-2\"")]
    #[serde(default)]
    pub doi: Option<String>,

    #[schemars(description = "PubMed ID")]
    #[serde(default)]
    pub pmid: Option<String>,

    #[schemars(
        description = "Structured identifiers: [{\"kind\":\"doi\",\"value\":\"...\"}]. \
                              Optional when `doi` or `pmid` top-level fields are provided."
    )]
    #[serde(default)]
    pub identifiers: Vec<IdentifierInput>,

    #[schemars(
        description = "Authors as display-name strings, e.g. [\"Smith J\", \"Doe K\"]. \
                              Matches the format returned by lit_search."
    )]
    #[serde(default)]
    pub authors: Vec<String>,

    #[schemars(description = "Publication year")]
    #[serde(default)]
    pub year: Option<u16>,

    #[schemars(description = "Journal or venue name")]
    #[serde(default)]
    pub journal: Option<String>,

    #[schemars(description = "Journal volume")]
    #[serde(default)]
    pub volume: Option<String>,

    #[schemars(description = "Issue number")]
    #[serde(default)]
    pub issue: Option<String>,

    #[schemars(description = "Page range (e.g. \"1-15\")")]
    #[serde(default)]
    pub pages: Option<String>,

    #[schemars(description = "Abstract text")]
    #[serde(default)]
    pub abstract_text: Option<String>,

    #[schemars(description = "Keywords (MeSH terms or author keywords)")]
    #[serde(default)]
    pub keywords: Vec<String>,

    #[schemars(description = "Publication types (e.g. [\"Journal Article\", \"Review\"])")]
    #[serde(default)]
    pub pub_types: Vec<String>,

    #[schemars(
        description = "Source this article came from (e.g. \"pubmed\", \"arxiv\", \
                              \"openalex\"). Used for provenance."
    )]
    #[serde(default)]
    pub source: Option<String>,
}

/// A structured identifier inside [`ArticleInput`].
#[derive(Debug, Clone, schemars::JsonSchema, serde::Deserialize, serde::Serialize)]
pub struct IdentifierInput {
    #[schemars(
        description = "Kind: \"doi\", \"pmid\", \"arxiv\", \"openalex\", \"s2\", \"biorxiv\""
    )]
    pub kind: String,
    #[schemars(description = "Identifier value")]
    pub value: String,
}

#[tool(
    name = "bib_save",
    description = "Save one or more articles to the local library. \
                  \
                  **Two modes** (provide exactly one): \
                  \
                  1. **`articles` (preferred)** — pass complete article metadata directly. \
                  No external fetch is needed; articles are stored immediately. This is the \
                  fast path: pass the JSON you already have from `lit_search` or `lit_fetch` \
                  results. \
                  \
                  2. **`ids`** — pass typed `{ id_type, id }` identifiers when you only have \
                  an ID and need the gateway to fetch metadata from external sources \
                  (PubMed, arXiv, OpenAlex, Crossref, Semantic Scholar, bioRxiv). \
                  \
                  Articles already in the library (matched by identifier) are returned as \
                  cached without re-fetching. \
                  \
                  After saving metadata, each newly stored article is **automatically checked** \
                  for an open-access full text on Europe PMC. When available it is downloaded \
                  (JATS XML → plain text) and stored alongside the article. Set \
                  `fetch_fulltext=false` to skip. \
                  \
                  **Examples**: \
                  • articles=[{title:\"...\", doi:\"10.1038/...\", year:2024}] — direct save \
                  • articles=[{title:\"...\", pmid:\"37658030\", authors:[\"Smith J\"]}] — direct save \
                  • ids=[{id_type:\"pmid\", id:\"37658030\"}] — fetch + save by PMID \
                  • ids=[{id_type:\"doi\", id:\"10.1038/...\"}, {id_type:\"arxiv\", id:\"2401.00001\"}] — batch fetch"
)]
pub struct BibSaveInput {
    #[desc = "Complete article metadata objects to save directly — no external fetch needed. \
             Pass the article JSON from lit_search or lit_fetch results. Each object needs \
             at least `title` and one identifier (doi, pmid, or identifiers array)."]
    pub articles: Option<Vec<ArticleInput>>,

    #[desc = "Typed identifiers to fetch + save (fetches metadata from external sources). \
             Use when you only have an ID without full metadata. Mutually exclusive with `articles`."]
    pub ids: Option<Vec<ArticleIdInput>>,

    #[desc = "Force a specific source for ALL ids (e.g. \"pubmed\", \"crossref\"). \
             Only used in `ids` mode. Default: auto-route each id to compatible sources."]
    pub source: Option<String>,
    #[desc = "Attempt to download open-access full text from Europe PMC after saving. Default: true"]
    pub fetch_fulltext: Option<bool>,
}

pub struct BibSaveTool {
    pub bib: Arc<BibBase>,
    pub gateway: Arc<LiteratureGateway>,
    /// Europe PMC client used for best-effort OA full-text auto-fetch.
    /// Defaults to [`EuropePmcClient::new`] when constructed via
    /// [`bib_library_registrations`].
    pub epmc: Arc<EuropePmcClient>,
    /// EasyScholar client used for best-effort journal-metrics enrichment
    /// (impact factor / quartiles). Constructed from `EASYSCHOLAR_KEY` by
    /// [`bib_library_registrations`]; without a key the enrichment is
    /// silently skipped.
    pub easyscholar: Arc<easyscholar::EasyscholarClient>,
}

/// Result of saving a single article within a batch.
#[derive(serde::Serialize)]
struct SaveResult {
    id: String,
    saved: bool,
    cached: bool,
    article_id: Option<String>,
    source: Option<String>,
    title: Option<String>,
    doi: Option<String>,
    pmid: Option<String>,
    year: Option<u16>,
    #[serde(default)]
    fulltext_fetched: bool,
    error: Option<String>,
}

#[async_trait]
impl ToolFunction for BibSaveTool {
    type Input = BibSaveInput;

    fn timeout_seconds(&self) -> u64 {
        // Generous timeout for batch fetches — each ID may require a network
        // round-trip to PubMed/arXiv. Capped at the agentik maximum.
        300
    }

    async fn run(&self, input: Self::Input) -> Result<AgentToolResult, ToolError> {
        let want_fulltext = input.fetch_fulltext.unwrap_or(true);

        // Determine mode: `articles` (direct save) or `ids` (fetch-by-id).
        let has_articles = input.articles.as_ref().is_some_and(|a| !a.is_empty());
        let has_ids = input.ids.as_ref().is_some_and(|i| !i.is_empty());

        if !has_articles && !has_ids {
            return Err(ToolError::ExecutionFailed {
                source: "provide either `articles` (full metadata, preferred) or `ids` \
                         (typed identifiers to fetch)"
                    .into(),
            });
        }

        let results = if has_articles {
            // ── Direct-save mode ───────────────────────────────────────────
            let articles = input.articles.unwrap();
            let futures: Vec<_> = articles
                .iter()
                .map(|a| self.save_article_direct(a, want_fulltext))
                .collect();
            futures::future::join_all(futures).await
        } else {
            // ── Fetch-by-id mode ───────────────────────────────────────────
            let ids = input.ids.unwrap();
            let mut identifiers: Vec<(String, Identifier)> = Vec::new();
            for entry in &ids {
                let kind =
                    parse_id_kind(&entry.id_type).ok_or_else(|| ToolError::ExecutionFailed {
                        source: format!(
                            "unknown id_type '{}': expected one of doi, pmid, arxiv, openalex, s2, biorxiv",
                            entry.id_type
                        )
                        .into(),
                    })?;
                identifiers.push((entry.id.clone(), Identifier::new(kind, entry.id.trim())));
            }

            let futures: Vec<_> = identifiers
                .iter()
                .map(|(raw, id)| self.save_one(raw, id, input.source.as_deref(), want_fulltext))
                .collect();
            futures::future::join_all(futures).await
        };

        let total = results.len();
        let saved = results.iter().filter(|r| r.saved && !r.cached).count();
        let cached = results.iter().filter(|r| r.cached).count();
        let failed = results.iter().filter(|r| !r.saved).count();
        let oa_count = results.iter().filter(|r| r.fulltext_fetched).count();

        Ok(AgentToolResult::success_json(serde_json::json!({
            "total": total,
            "saved": saved,
            "cached": cached,
            "failed": failed,
            "oa_fulltext_fetched": oa_count,
            "results": results,
            "message": format!(
                "{saved} saved, {cached} cached, {failed} failed \
                 ({oa_count} OA full texts fetched from Europe PMC) \
                 out of {total} articles."
            ),
        })))
    }
}

impl BibSaveTool {
    /// Save a full article directly — no external fetch.  Never errors;
    /// failures are captured in the returned [`SaveResult::error`].
    async fn save_article_direct(&self, input: &ArticleInput, want_fulltext: bool) -> SaveResult {
        // Build the canonical Article from the input.
        let article = match article_input_to_article(input) {
            Ok(a) => a,
            Err(msg) => {
                return SaveResult {
                    id: input.title.clone(),
                    saved: false,
                    cached: false,
                    article_id: None,
                    source: input.source.clone(),
                    title: Some(input.title.clone()),
                    doi: input.doi.clone(),
                    pmid: input.pmid.clone(),
                    year: input.year,
                    fulltext_fetched: false,
                    error: Some(msg),
                };
            }
        };

        let raw_id = article
            .doi()
            .or(article.pmid())
            .map(str::to_owned)
            .unwrap_or_else(|| article.title.clone());

        // Check if already in local library (dedup by any identifier).
        let existing_identifier = article.identifiers.first();
        if let Some(id) = existing_identifier {
            if let Ok(Some(existing)) = self.bib.find_by_identifier(id.kind, &id.value).await {
                let has_ft = self.bib.has_fulltext(&existing.id).await.unwrap_or(false);
                let cached_doi = existing.doi().map(str::to_owned);
                let cached_pmid = existing.pmid().map(str::to_owned);
                return SaveResult {
                    id: raw_id,
                    saved: true,
                    cached: true,
                    article_id: Some(existing.id),
                    source: input.source.clone(),
                    title: Some(existing.title),
                    doi: cached_doi,
                    pmid: cached_pmid,
                    year: existing.year,
                    fulltext_fetched: has_ft,
                    error: None,
                };
            }
        }

        let article_id = article.id.clone();
        let title = article.title.clone();
        let doi = article.doi().map(str::to_owned);
        let pmid = article.pmid().map(str::to_owned);
        let year = article.year;

        match self.bib.upsert_article(&article).await {
            Ok(()) => {
                let ft_fetched = if want_fulltext {
                    self.try_fetch_oa_fulltext(&article).await
                } else {
                    false
                };
                SaveResult {
                    id: raw_id,
                    saved: true,
                    cached: false,
                    article_id: Some(article_id),
                    source: input.source.clone(),
                    title: Some(title),
                    doi,
                    pmid,
                    year,
                    fulltext_fetched: ft_fetched,
                    error: None,
                }
            }
            Err(e) => SaveResult {
                id: raw_id,
                saved: false,
                cached: false,
                article_id: None,
                source: input.source.clone(),
                title: Some(title),
                doi,
                pmid,
                year,
                fulltext_fetched: false,
                error: Some(format!("Database error: {e}")),
            },
        }
    }

    /// Fetch + upsert a single article. Never errors — failures are captured
    /// in the returned [`SaveResult::error`] so one bad ID doesn't abort the batch.
    async fn save_one(
        &self,
        raw_id: &str,
        identifier: &Identifier,
        source_override: Option<&str>,
        want_fulltext: bool,
    ) -> SaveResult {
        // 1. Check if already in local library.
        if let Ok(Some(existing)) = self
            .bib
            .find_by_identifier(identifier.kind, &identifier.value)
            .await
        {
            let doi = existing.doi().map(str::to_owned);
            let pmid = existing.pmid().map(str::to_owned);

            // If cached and already has full text, report it.
            let has_ft = self.bib.has_fulltext(&existing.id).await.unwrap_or(false);

            return SaveResult {
                id: raw_id.into(),
                saved: true,
                cached: true,
                article_id: Some(existing.id),
                source: None,
                title: Some(existing.title),
                doi,
                pmid,
                year: existing.year,
                fulltext_fetched: has_ft,
                error: None,
            };
        }

        // 2. Fetch from external source.
        let fetched = if let Some(source) = source_override {
            self.gateway
                .fetch_from(source, identifier)
                .await
                .ok()
                .flatten()
                .map(|article| (source.into(), article))
        } else {
            self.gateway.fetch(identifier).await
        };

        let (source_name, article) = match fetched {
            Some(x) => x,
            None => {
                return SaveResult {
                    id: raw_id.into(),
                    saved: false,
                    cached: false,
                    article_id: None,
                    source: None,
                    title: None,
                    doi: None,
                    pmid: None,
                    year: None,
                    fulltext_fetched: false,
                    error: Some(format!(
                        "Article '{}:{}' not found in any compatible source",
                        identifier.kind.as_str(),
                        raw_id
                    )),
                };
            }
        };

        // 3. Upsert to local DB.
        let article_id = article.id.clone();
        let title = article.title.clone();
        let doi = article.doi().map(str::to_owned);
        let pmid = article.pmid().map(str::to_owned);
        let year = article.year;

        match self.bib.upsert_article(&article).await {
            Ok(()) => {
                // 4. Best-effort OA full-text fetch from Europe PMC.
                let ft_fetched = if want_fulltext {
                    self.try_fetch_oa_fulltext(&article).await
                } else {
                    false
                };

                // 5. Best-effort journal-metrics enrichment (EasyScholar).
                // Inline rather than spawned: the tool's 300s timeout dwarfs
                // the client's 10s cap, and returning with metrics already
                // cached keeps the agent from observing a half-enriched save.
                if let Some(journal) = article
                    .journal
                    .as_deref()
                    .map(str::trim)
                    .filter(|j| !j.is_empty())
                {
                    let _ = crate::journal_metrics::enrich_journal_metrics(
                        &self.bib,
                        &self.easyscholar,
                        journal,
                    )
                    .await;
                }

                SaveResult {
                    id: raw_id.into(),
                    saved: true,
                    cached: false,
                    article_id: Some(article_id),
                    source: Some(source_name),
                    title: Some(title),
                    doi,
                    pmid,
                    year,
                    fulltext_fetched: ft_fetched,
                    error: None,
                }
            }
            Err(e) => SaveResult {
                id: raw_id.into(),
                saved: false,
                cached: false,
                article_id: None,
                source: Some(source_name),
                title: Some(title),
                doi,
                pmid,
                year,
                fulltext_fetched: false,
                error: Some(format!("Database error: {e}")),
            },
        }
    }

    /// Attempt to fetch an open-access full text for `article` from Europe PMC
    /// and store it. Returns `true` on success.
    async fn try_fetch_oa_fulltext(&self, article: &bib_types::Article) -> bool {
        let ft = match try_fetch_fulltext_with(&self.epmc, article).await {
            Some(ft) => ft,
            None => return false,
        };
        match self.bib.upsert_fulltext(&ft).await {
            Ok(()) => true,
            Err(e) => {
                tracing::warn!(article_id = %article.id, error = %e, "failed to store OA full text");
                false
            }
        }
    }
}

// ---------------------------------------------------------------------------
// ArticleInput → Article conversion
// ---------------------------------------------------------------------------

/// Convert an [`ArticleInput`] (agent-facing DTO) into a canonical
/// [`bib_types::Article`].
///
/// Top-level `doi`/`pmid` fields are merged into `identifiers` alongside
/// any structured `identifiers` array (duplicates skipped).  Author
/// display-name strings are parsed into [`bib_types::Author`] structs.
fn article_input_to_article(input: &ArticleInput) -> Result<bib_types::Article, String> {
    use bib_types::{Article, ArticleSource, Author, IdKind, Identifier};

    if input.title.trim().is_empty() {
        return Err("article `title` is required and must not be empty".into());
    }

    // Collect identifiers from both structured array and top-level convenience fields.
    let mut identifiers: Vec<Identifier> = Vec::new();
    let mut seen: std::collections::HashSet<(IdKind, String)> = std::collections::HashSet::new();

    // Helper closure to add without duplicating.
    let mut push_id = |kind: IdKind, value: &str, ids: &mut Vec<Identifier>| {
        let v = value.trim().to_owned();
        if v.is_empty() {
            return;
        }
        if seen.insert((kind, v.clone())) {
            ids.push(Identifier::new(kind, v));
        }
    };

    // Structured identifiers.
    for id_input in &input.identifiers {
        let kind = parse_id_kind(&id_input.kind).unwrap_or(IdKind::Other);
        push_id(kind, &id_input.value, &mut identifiers);
    }
    // Convenience top-level fields.
    if let Some(ref doi) = input.doi {
        push_id(IdKind::Doi, doi, &mut identifiers);
    }
    if let Some(ref pmid) = input.pmid {
        push_id(IdKind::Pmid, pmid, &mut identifiers);
    }

    if identifiers.is_empty() {
        return Err("at least one identifier is required (doi, pmid, or identifiers array)".into());
    }

    // Parse authors: "Family Given" or "Family" from display-name strings.
    let authors: Vec<Author> = input
        .authors
        .iter()
        .filter(|s| !s.trim().is_empty())
        .map(|display| {
            let trimmed = display.trim();
            // Try to split into last + rest at the first space.
            if let Some(space) = trimmed.find(' ') {
                let (last, rest) = trimmed.split_at(space);
                Author {
                    last_name: last.to_owned(),
                    fore_name: Some(rest.trim().to_owned()),
                    initials: None,
                    affiliation: None,
                    orcid: None,
                    corresponding: false,
                }
            } else {
                Author {
                    last_name: trimmed.to_owned(),
                    fore_name: None,
                    initials: None,
                    affiliation: None,
                    orcid: None,
                    corresponding: false,
                }
            }
        })
        .collect();

    // Derive the internal article ID from the primary identifier.
    let primary = &identifiers[0];
    let id = format!("{}:{}", primary.kind.as_str(), primary.value);

    // Parse source.
    let source = input
        .source
        .as_deref()
        .map(|s| match s.to_lowercase().as_str() {
            "pubmed" => ArticleSource::Pubmed,
            "arxiv" => ArticleSource::Arxiv,
            "biorxiv" => ArticleSource::Biorxiv,
            "openalex" => ArticleSource::OpenAlex,
            "crossref" | "doi" => ArticleSource::CrossRef,
            "europepmc" | " europe_pmc" => ArticleSource::EuropePmc,
            "semantic_scholar" | "s2" => ArticleSource::SemanticScholar,
            "gwascatalog" => ArticleSource::GwasCatalog,
            "manual" => ArticleSource::Manual,
            _ => ArticleSource::Unknown,
        })
        .unwrap_or(ArticleSource::Unknown);

    let now = chrono::Utc::now();
    Ok(Article {
        id,
        title: input.title.trim().to_owned(),
        authors,
        identifiers,
        abstract_text: input.abstract_text.clone().filter(|s| !s.is_empty()),
        year: input.year,
        month: None,
        journal: input.journal.clone().filter(|s| !s.is_empty()),
        volume: input.volume.clone().filter(|s| !s.is_empty()),
        issue: input.issue.clone().filter(|s| !s.is_empty()),
        pages: input.pages.clone().filter(|s| !s.is_empty()),
        issn: None,
        essn: None,
        language: None,
        pub_types: input.pub_types.clone(),
        keywords: input.keywords.clone(),
        source,
        created_at: Some(now),
        updated_at: Some(now),
    })
}

// ===========================================================================
// bib_create_collection
// ===========================================================================

#[tool(
    name = "bib_create_collection",
    description = "Create a new literature collection (a research investigation, reading \
                  list, or project bibliography). A auto-generated ID is returned for \
                  use in subsequent calls (bib_add_to_collection, bib_list_collection, \
                  etc.)."
)]
pub struct BibCreateCollectionInput {
    #[desc = "Human-readable collection name (e.g. \"eQTL colocalization methods\")"]
    pub name: String,
    #[desc = "Optional description of the collection's purpose"]
    pub description: Option<String>,
}

pub struct BibCreateCollectionTool {
    pub bib: Arc<BibBase>,
}

#[async_trait]
impl ToolFunction for BibCreateCollectionTool {
    type Input = BibCreateCollectionInput;

    async fn run(&self, input: Self::Input) -> Result<AgentToolResult, ToolError> {
        let id = format!("col-{}", &uuid::Uuid::new_v4().to_string()[..8]);

        let mut col = bib_types::Collection::new(&id, &input.name);
        col.description = input.description;

        self.bib.upsert_collection(&col).await.map_err(box_error)?;

        Ok(AgentToolResult::success_json(serde_json::json!({
            "collection_id": id,
            "name": col.name,
            "description": col.description,
            "status": "active",
        })))
    }
}

// ===========================================================================
// bib_add_to_collection — batch
// ===========================================================================

/// One article to add to a collection. Role/note are optional and fall back
/// to the top-level `default_role` / `default_note` on [`BibAddToCollectionInput`].
#[derive(serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct BibAddToCollectionEntry {
    /// Article ID (from bib_save or lit_search results).
    pub article_id: String,
    /// Role for this article: "requested", "referenced", "cited", or "background".
    /// When omitted the top-level `default_role` is used.
    pub role: Option<String>,
    /// Note explaining this article's relevance to the collection.
    /// When omitted the top-level `default_note` is used.
    pub note: Option<String>,
}

#[tool(
    name = "bib_add_to_collection",
    description = "Add one or more articles (already saved via bib_save) to a collection, \
                  each with a semantic role describing why it's included. \
                  \
                  Pass a list under `articles`; a single-article call is just a \
                  one-element list. Each entry may carry its own `role` and `note`; \
                  entries that omit them inherit the top-level `default_role` / \
                  `default_note`. \
                  \
                  If an article is already in the collection, its entry is updated in \
                  place (not duplicated). Each result reports `action:\"inserted\"` for \
                  a new entry or `action:\"updated\"` with `role_changed` / \
                  `note_changed` flags for an overwrite. Omitting `note` (both on the \
                  entry and at the top level) preserves the existing note; passing an \
                  explicit note overwrites it. \
                  \
                  Roles: \
                  • \"requested\" — you actively need this paper for your investigation \
                  • \"referenced\" — supporting context \
                  • \"cited\" — cited in the collection's output \
                  • \"background\" — general background knowledge (default)"
)]
pub struct BibAddToCollectionInput {
    #[desc = "Collection ID (from bib_create_collection)"]
    pub collection_id: String,
    #[desc = "Articles to add. Pass a list — even for a single article use a one-element list."]
    pub articles: Vec<BibAddToCollectionEntry>,
    #[desc = "Default role for entries that omit their own: \"requested\", \"referenced\", \"cited\", or \"background\". Default: referenced"]
    pub default_role: Option<String>,
    #[desc = "Default note for entries that omit their own note."]
    pub default_note: Option<String>,
}

pub struct BibAddToCollectionTool {
    pub bib: Arc<BibBase>,
}

/// Per-article outcome within a batch add.
#[derive(serde::Serialize)]
struct AddResult {
    article_id: String,
    added: bool,
    action: &'static str,
    role: String,
    note: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    previous_role: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    previous_note: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    role_changed: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    note_changed: Option<bool>,
    error: Option<String>,
}

#[async_trait]
impl ToolFunction for BibAddToCollectionTool {
    type Input = BibAddToCollectionInput;

    async fn run(&self, input: Self::Input) -> Result<AgentToolResult, ToolError> {
        if input.articles.is_empty() {
            return Err(ToolError::ExecutionFailed {
                source: "at least one article is required".into(),
            });
        }

        let default_role = parse_role(input.default_role.as_deref());

        // Resolve each entry's role/note against the top-level defaults, then
        // add it sequentially (position ordering matters within a collection).
        let mut results = Vec::with_capacity(input.articles.len());
        for entry in &input.articles {
            results.push(
                self.add_one(
                    &input.collection_id,
                    entry,
                    default_role,
                    input.default_note.as_deref(),
                )
                .await,
            );
        }

        let total = results.len();
        let inserted = results
            .iter()
            .filter(|r| r.action == "inserted" && r.error.is_none())
            .count();
        let updated = results
            .iter()
            .filter(|r| r.action == "updated" && r.error.is_none())
            .count();
        let failed = results.iter().filter(|r| r.error.is_some()).count();

        Ok(AgentToolResult::success_json(serde_json::json!({
            "collection_id": input.collection_id,
            "total": total,
            "inserted": inserted,
            "updated": updated,
            "failed": failed,
            "results": results,
            "message": format!(
                "{inserted} inserted, {updated} updated, {failed} failed \
                 out of {total} articles in collection '{}'.",
                input.collection_id
            ),
        })))
    }
}

impl BibAddToCollectionTool {
    /// Add a single article to a collection, never erroring — failures are
    /// captured in the returned [`AddResult::error`] so one bad article
    /// doesn't abort the batch.
    async fn add_one(
        &self,
        collection_id: &str,
        entry: &BibAddToCollectionEntry,
        default_role: ArticleRole,
        default_note: Option<&str>,
    ) -> AddResult {
        // Entry role takes priority; fall back to the top-level default. Resolve
        // at the string level before parsing so an explicit "referenced" is not
        // confused with an omitted role (both map to ArticleRole::Referenced).
        let role = match entry.role.as_deref() {
            Some(r) => parse_role(Some(r)),
            None => default_role,
        };
        // Entry note takes priority; fall back to the top-level default.
        let note = entry.note.as_deref().or(default_note);

        let outcome = self
            .bib
            .add_to_collection(collection_id, &entry.article_id, role, AddedBy::Agent, note)
            .await;

        match outcome {
            Ok(CollectionAddOutcome::Inserted) => AddResult {
                article_id: entry.article_id.clone(),
                added: true,
                action: "inserted",
                role: role.as_str().to_owned(),
                note: note.map(str::to_owned),
                previous_role: None,
                previous_note: None,
                role_changed: None,
                note_changed: None,
                error: None,
            },
            Ok(CollectionAddOutcome::Updated {
                previous_role,
                previous_note,
                role_changed,
                note_changed,
            }) => AddResult {
                article_id: entry.article_id.clone(),
                added: true,
                action: "updated",
                role: role.as_str().to_owned(),
                note: note.map(str::to_owned).or(previous_note.clone()),
                previous_role: Some(previous_role.as_str().to_owned()),
                previous_note,
                role_changed: Some(role_changed),
                note_changed: Some(note_changed),
                error: None,
            },
            Err(e) => AddResult {
                article_id: entry.article_id.clone(),
                added: false,
                action: "error",
                role: role.as_str().to_owned(),
                note: note.map(str::to_owned),
                previous_role: None,
                previous_note: None,
                role_changed: None,
                note_changed: None,
                error: Some(e.to_string()),
            },
        }
    }
}

// ===========================================================================
// bib_list_collection — dual-purpose: list collections OR list articles
// ===========================================================================

#[tool(
    name = "bib_list_collection",
    description = "List collections in the library, or list articles within a specific \
                  collection. \
                  \
                  • Omit `collection_id` → list all collections (with status + article count). \
                  • Provide `collection_id` → list articles in that collection. \
                  \
                  Optionally filter by role or fetch_status when listing articles."
)]
pub struct BibListCollectionInput {
    #[desc = "Collection ID. If omitted, lists all collections instead."]
    pub collection_id: Option<String>,
    #[desc = "Filter by role: \"requested\", \"referenced\", \"cited\", \"background\""]
    pub role: Option<String>,
    #[desc = "Filter by fetch status: \"metadata_only\", \"fulltext_requested\", \"fulltext_available\""]
    pub fetch_status: Option<String>,
}

pub struct BibListCollectionTool {
    pub bib: Arc<BibBase>,
}

#[async_trait]
impl ToolFunction for BibListCollectionTool {
    type Input = BibListCollectionInput;

    async fn run(&self, input: Self::Input) -> Result<AgentToolResult, ToolError> {
        match input.collection_id {
            None => {
                // List all collections.
                let collections = self.bib.list_collections(None).await.map_err(box_error)?;

                let items: Vec<serde_json::Value> = collections
                    .iter()
                    .map(|c| {
                        serde_json::json!({
                            "collection_id": c.id,
                            "name": c.name,
                            "description": c.description,
                            "status": c.status.as_str(),
                            "n_articles": c.article_ids.len(),
                            "tags": c.tags,
                        })
                    })
                    .collect();

                Ok(AgentToolResult::success_json(serde_json::json!({
                    "total": items.len(),
                    "collections": items,
                })))
            }
            Some(cid) => {
                // List articles in a collection.
                let role_filter = input.role.as_deref().and_then(parse_role_opt);
                let fetch_filter = input
                    .fetch_status
                    .as_deref()
                    .and_then(parse_fetch_status_opt);

                let articles = self
                    .bib
                    .list_collection_articles(&cid, role_filter, fetch_filter)
                    .await
                    .map_err(box_error)?;

                // Enrich with article metadata from local DB.
                let mut items = Vec::with_capacity(articles.len());
                for ca in &articles {
                    let article = self
                        .bib
                        .get_article(&ca.article_id)
                        .await
                        .map_err(box_error)?;
                    let entry = match article {
                        Some(a) => serde_json::json!({
                            "article_id": ca.article_id,
                            "title": a.title,
                            "year": a.year,
                            "journal": a.journal,
                            "doi": a.doi(),
                            "pmid": a.pmid(),
                            "role": ca.role.as_str(),
                            "fetch_status": ca.fetch_status.as_str(),
                            "note": ca.note,
                        }),
                        None => serde_json::json!({
                            "article_id": ca.article_id,
                            "title": "(metadata missing)",
                            "role": ca.role.as_str(),
                            "fetch_status": ca.fetch_status.as_str(),
                        }),
                    };
                    items.push(entry);
                }

                // Get collection name.
                let collection = self.bib.get_collection(&cid).await.map_err(box_error)?;

                Ok(AgentToolResult::success_json(serde_json::json!({
                    "collection_id": cid,
                    "collection_name": collection.as_ref().map(|c| c.name.as_str()).unwrap_or("(unknown)"),
                    "total": items.len(),
                    "articles": items,
                })))
            }
        }
    }
}

// ===========================================================================
// bib_search_library — LIKE search across local library (multi-query, concurrent)
// ===========================================================================

/// Result of a single query within a multi-query batch.
#[derive(serde::Serialize)]
struct SearchQueryResult {
    query: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    total: usize,
    results: Vec<serde_json::Value>,
}

#[tool(
    name = "bib_search_library",
    description = "Search the local library for articles by keyword. Searches across \
                  titles, abstracts, stored full-text content, **and user annotations** \
                  (notes/highlights/comments added via bib_add_note). \
                  Returns relevance-ranked results with snippets. \
                  \
                  Pass one or more queries — each is run concurrently against the local \
                  database and returns its own result group. Use multiple queries when \
                  you want to find articles matching several independent terms in one \
                  call instead of N separate tool calls. \
                  \
                  Use this to find articles you've already saved (via bib_save). \
                  For searching external databases, use lit_search instead. \
                  \
                  **Examples**: \
                  • queries=[\"Mendelian randomization\"] — single search \
                  • queries=[\"GWAS\", \"polygenic risk score\", \"LDSC\"] — multi-search (concurrent)"
)]
pub struct BibSearchLibraryInput {
    #[desc = "One or more search queries (each matched against title, abstract, full text, and annotations)"]
    pub queries: Vec<String>,
    #[desc = "Maximum results per query (default 20)"]
    pub limit: Option<usize>,
}

pub struct BibSearchLibraryTool {
    pub bib: Arc<BibBase>,
}

#[async_trait]
impl ToolFunction for BibSearchLibraryTool {
    type Input = BibSearchLibraryInput;

    async fn run(&self, input: Self::Input) -> Result<AgentToolResult, ToolError> {
        if input.queries.is_empty() {
            return Err(ToolError::ExecutionFailed {
                source: "at least one search query is required".into(),
            });
        }

        let limit = input.limit.unwrap_or(20).clamp(1, 100);

        // Run each query concurrently; failures are isolated per query so
        // one malformed term doesn't fail the whole batch.
        let futures: Vec<_> = input
            .queries
            .iter()
            .map(|q| self.search_one(q.trim(), limit))
            .collect();
        let groups = futures::future::join_all(futures).await;

        let n_queries = groups.len();
        let total_hits: usize = groups.iter().map(|g| g.total).sum();
        let failed = groups.iter().filter(|g| g.error.is_some()).count();

        Ok(AgentToolResult::success_json(serde_json::json!({
            "n_queries": n_queries,
            "total_hits": total_hits,
            "failed_queries": failed,
            "groups": groups,
            "message": format!(
                "{n_queries} queries, {total_hits} total hits, {failed} failed."
            ),
        })))
    }
}

impl BibSearchLibraryTool {
    /// Run a single LIKE query. Never errors — failures land in
    /// `SearchQueryResult::error`.
    async fn search_one(&self, query: &str, limit: usize) -> SearchQueryResult {
        let hits = match self.bib.search_articles(query, limit).await {
            Ok(h) => h,
            Err(e) => {
                return SearchQueryResult {
                    query: query.to_owned(),
                    error: Some(e.to_string()),
                    total: 0,
                    results: Vec::new(),
                };
            }
        };

        let results: Vec<serde_json::Value> = hits
            .iter()
            .map(|h| {
                serde_json::json!({
                    "article_id": h.article_id,
                    "title": h.title,
                    "snippet": h.snippet,
                })
            })
            .collect();
        let total = results.len();

        SearchQueryResult {
            query: query.to_owned(),
            error: None,
            total,
            results,
        }
    }
}

// ===========================================================================
// bib_get_article — full metadata + full text from local library (batch)
// ===========================================================================

/// Result of fetching a single article within a batch. Never errors at the
/// tool level — failures (including "not found") are captured in `error` so
/// one bad ID doesn't abort the whole batch. Mirrors the `SaveResult` pattern
/// used by `bib_save`.
#[derive(serde::Serialize)]
struct GetArticleResult {
    article_id: String,
    found: bool,
    title: Option<String>,
    authors: Vec<String>,
    year: Option<u16>,
    month: Option<u8>,
    journal: Option<String>,
    volume: Option<String>,
    issue: Option<String>,
    pages: Option<String>,
    doi: Option<String>,
    pmid: Option<String>,
    identifiers: Vec<serde_json::Value>,
    #[serde(rename = "abstract", skip_serializing_if = "Option::is_none")]
    abstract_text: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    keywords: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub_types: Vec<String>,
    has_fulltext: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    fulltext_source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    fulltext: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    fulltext_offset: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    fulltext_limit: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    fulltext_total_chars: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    fulltext_next_offset: Option<usize>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    annotations: Vec<serde_json::Value>,
    n_annotations: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

impl GetArticleResult {
    /// Build an error/missing stub for a requested ID. All data fields are
    /// empty; `found` is false and `error` carries the reason.
    fn err(article_id: impl Into<String>, found: bool, msg: impl Into<String>) -> Self {
        Self {
            article_id: article_id.into(),
            found,
            title: None,
            authors: Vec::new(),
            year: None,
            month: None,
            journal: None,
            volume: None,
            issue: None,
            pages: None,
            doi: None,
            pmid: None,
            identifiers: Vec::new(),
            abstract_text: None,
            keywords: Vec::new(),
            pub_types: Vec::new(),
            has_fulltext: false,
            fulltext_source: None,
            fulltext: None,
            fulltext_offset: None,
            fulltext_limit: None,
            fulltext_total_chars: None,
            fulltext_next_offset: None,
            annotations: Vec::new(),
            n_annotations: 0,
            error: Some(msg.into()),
        }
    }
}

#[tool(
    name = "bib_get_article",
    description = "Retrieve one or more articles from the local library, including metadata \
                  (title, authors, abstract, journal, identifiers), stored full text \
                  if available, and all user annotations (notes/highlights/comments). \
                  \
                  Pass a list of article IDs — each is fetched concurrently from the \
                  local database. Articles must have been saved via `bib_save` first; \
                  missing IDs are returned with `found: false` rather than failing the \
                  whole call. \
                  \
                  Full-text content is omitted by default to keep responses and agent \
                  context bounded. Set `include_fulltext=true` to receive one bounded \
                  page; use `offset` and `next_offset` to read additional pages. \
                  \
                  **Examples**: \
                  • article_ids=[\"a1\"] — fetch a single article \
                  • article_ids=[\"a1\", \"a2\", \"a3\"] — batch (fetched concurrently)"
)]
pub struct BibGetArticleInput {
    #[desc = "One or more article IDs (from bib_save or bib_search_library results)"]
    pub article_ids: Vec<String>,
    #[desc = "If true and full text is available, include one bounded full-text page. Default: false"]
    pub include_fulltext: Option<bool>,
    #[desc = "Character offset for the requested full-text page. Default: 0"]
    pub offset: Option<usize>,
    #[desc = "Maximum characters per result when include_fulltext=true. Default: 50000; max: 250000"]
    pub limit: Option<usize>,
}

pub struct BibGetArticleTool {
    pub bib: Arc<BibBase>,
}

#[async_trait]
impl ToolFunction for BibGetArticleTool {
    type Input = BibGetArticleInput;

    async fn run(&self, input: Self::Input) -> Result<AgentToolResult, ToolError> {
        if input.article_ids.is_empty() {
            return Err(ToolError::ExecutionFailed {
                source: "at least one article ID is required".into(),
            });
        }

        let include_ft = input.include_fulltext.unwrap_or(false);
        let offset = input.offset.unwrap_or(0);
        let limit = input.limit.unwrap_or(50_000).clamp(1, 250_000);

        // Fetch each article concurrently. The local DB is fast, but batching
        // still collapses N tool-call round-trips into one and lets the reads
        // overlap on the connection pool.
        let futures: Vec<_> = input
            .article_ids
            .iter()
            .map(|id| self.get_one(id.trim(), include_ft, offset, limit))
            .collect();
        let results = futures::future::join_all(futures).await;

        let total = results.len();
        let found = results.iter().filter(|r| r.found).count();
        let missing = total - found;
        let with_fulltext = results.iter().filter(|r| r.has_fulltext).count();

        Ok(AgentToolResult::success_json(serde_json::json!({
            "total": total,
            "found": found,
            "missing": missing,
            "with_fulltext": with_fulltext,
            "results": results,
            "message": format!(
                "{found} found, {missing} missing, {with_fulltext} with full text \
                 out of {total} requested."
            ),
        })))
    }
}

impl BibGetArticleTool {
    /// Fetch a single article + its full text + annotations. Never errors —
    /// failures are captured in `GetArticleResult::error` so one bad ID
    /// doesn't abort the batch.
    async fn get_one(
        &self,
        article_id: &str,
        include_ft: bool,
        offset: usize,
        limit: usize,
    ) -> GetArticleResult {
        let article = match self.bib.get_article(article_id).await {
            Ok(Some(a)) => a,
            Ok(None) => {
                return GetArticleResult::err(
                    article_id,
                    false,
                    format!(
                        "Article '{article_id}' not found in local library. \
                         Use bib_save to add it first."
                    ),
                );
            }
            Err(e) => return GetArticleResult::err(article_id, false, e.to_string()),
        };

        // `has_fulltext` must reflect the true DB state regardless of whether
        // the caller asked for the content. Previously it was derived from
        // `fulltext.is_some()`, so `include_fulltext=false` falsely reported
        // `has_fulltext:false` for articles that did have a full text.
        let fulltext_page = if include_ft {
            match self.bib.get_fulltext_page(article_id, offset, limit).await {
                Ok(page) => page,
                Err(e) => return GetArticleResult::err(article_id, false, e.to_string()),
            }
        } else {
            None
        };
        let has_fulltext = if include_ft {
            fulltext_page.is_some()
        } else {
            match self.bib.has_fulltext(article_id).await {
                Ok(found) => found,
                Err(e) => return GetArticleResult::err(article_id, false, e.to_string()),
            }
        };
        let fulltext_source = fulltext_page
            .as_ref()
            .map(|page| page.fulltext.source.as_str().to_owned());
        let fulltext_total_chars = fulltext_page.as_ref().map(|page| page.total_chars);
        let fulltext_next_offset = fulltext_page.as_ref().and_then(|page| page.next_offset);
        let fulltext = fulltext_page.and_then(|page| page.fulltext.text_content);

        // Annotations (notes/highlights/comments) — without this the write
        // path (bib_add_note) is a data black hole: annotations are persisted
        // but never reachable through any read tool.
        let annotations = match self.bib.list_annotations(article_id).await {
            Ok(a) => a,
            Err(e) => return GetArticleResult::err(article_id, false, e.to_string()),
        };
        let n_annotations = annotations.len();

        // Borrow-derived values first, before any `article` fields are moved.
        let doi = article.doi().map(str::to_owned);
        let pmid = article.pmid().map(str::to_owned);
        let authors: Vec<String> = article.authors.iter().map(|a| a.display_name()).collect();
        let identifiers: Vec<serde_json::Value> = article
            .identifiers
            .iter()
            .map(|i| serde_json::json!({"kind": i.kind.as_str(), "value": i.value}))
            .collect();

        GetArticleResult {
            article_id: article.id.clone(),
            found: true,
            title: Some(article.title),
            authors,
            year: article.year,
            month: article.month,
            journal: article.journal,
            volume: article.volume,
            issue: article.issue,
            pages: article.pages,
            doi,
            pmid,
            identifiers,
            abstract_text: article.abstract_text,
            keywords: article.keywords.clone(),
            pub_types: article.pub_types.clone(),
            has_fulltext,
            fulltext_source,
            fulltext,
            fulltext_offset: if include_ft { Some(offset) } else { None },
            fulltext_limit: if include_ft { Some(limit) } else { None },
            fulltext_total_chars,
            fulltext_next_offset,
            annotations: annotations
                .iter()
                .map(|a| {
                    serde_json::json!({
                        "id": a.id,
                        "kind": a.kind.as_str(),
                        "content": a.content,
                        "page": a.page,
                        "created_at": a.created_at.map(|t| t.to_rfc3339()),
                    })
                })
                .collect(),
            n_annotations,
            error: None,
        }
    }
}

// ===========================================================================
// bib_request_fulltext — mark as needing full-text upload
// ===========================================================================

#[tool(
    name = "bib_request_fulltext",
    description = "Mark an article as needing full-text upload within a collection. \
                  The article's fetch_status changes to \"fulltext_requested\", signaling \
                  the user to provide a PDF. \
                  \
                  After the user uploads the full text, the article's status becomes \
                  \"fulltext_available\"; bib_get_article can then read bounded pages."
)]
pub struct BibRequestFulltextInput {
    #[desc = "Collection ID (the collection context for this request)"]
    pub collection_id: String,
    #[desc = "Article ID (already in the local library via bib_save)"]
    pub article_id: String,
}

pub struct BibRequestFulltextTool {
    pub bib: Arc<BibBase>,
}

#[async_trait]
impl ToolFunction for BibRequestFulltextTool {
    type Input = BibRequestFulltextInput;

    async fn run(&self, input: Self::Input) -> Result<AgentToolResult, ToolError> {
        self.bib
            .update_fetch_status(
                &input.collection_id,
                &input.article_id,
                FetchStatus::FulltextRequested,
            )
            .await
            .map_err(box_error)?;

        Ok(AgentToolResult::success_json(serde_json::json!({
            "requested": true,
            "collection_id": input.collection_id,
            "article_id": input.article_id,
            "fetch_status": "fulltext_requested",
            "message": "Article marked as needing full text. The user can upload a PDF to fulfill this request.",
        })))
    }
}

// ===========================================================================
// bib_add_note — add annotation to article
// ===========================================================================

#[tool(
    name = "bib_add_note",
    description = "Add a note, highlight, or comment to an article in the library. \
                  Useful for recording observations, key findings, or critique while \
                  reading a paper."
)]
pub struct BibAddNoteInput {
    #[desc = "Article ID (from bib_save or bib_search_library)"]
    pub article_id: String,
    #[desc = "Annotation type: \"note\", \"highlight\", or \"comment\". Default: note"]
    pub kind: Option<String>,
    #[desc = "The annotation content (note text, highlighted excerpt, or comment)"]
    pub content: String,
    #[desc = "PDF page number (1-based) if the annotation is anchored to a page"]
    pub page: Option<u32>,
}

pub struct BibAddNoteTool {
    pub bib: Arc<BibBase>,
}

#[async_trait]
impl ToolFunction for BibAddNoteTool {
    type Input = BibAddNoteInput;

    async fn run(&self, input: Self::Input) -> Result<AgentToolResult, ToolError> {
        let kind = match input.kind.as_deref().map(|s| s.to_lowercase()).as_deref() {
            Some("highlight") => bib_types::AnnotationKind::Highlight,
            Some("comment") => bib_types::AnnotationKind::Comment,
            _ => bib_types::AnnotationKind::Note,
        };

        let ann = self
            .bib
            .add_annotation(&input.article_id, kind, &input.content, input.page, None)
            .await
            .map_err(box_error)?;

        Ok(AgentToolResult::success_json(serde_json::json!({
            "annotation_id": ann.id,
            "article_id": input.article_id,
            "kind": ann.kind.as_str(),
            "content": ann.content,
            "page": ann.page,
        })))
    }
}

// ===========================================================================
// bib_delete — remove articles from the local library
// ===========================================================================

/// Per-article result row for [`BibDeleteTool`].
#[derive(serde::Serialize)]
struct DeleteResult {
    article_id: String,
    /// `true` only when a row was actually removed from `articles` during
    /// this call. Always `false` in dry-run mode (`confirm=false`) and
    /// `false` when the id did not match any row in commit mode.
    deleted: bool,
    /// `true` if the article row existed at lookup time — useful to
    /// distinguish "id not found" from "id found and removed".
    found: bool,
    title: Option<String>,
    /// Non-null only when the row could not be deleted (storage error or
    /// id-not-found in commit mode). Not used in dry-run.
    error: Option<String>,
}

/// Input for [`BibDeleteTool`] — the only path that physically removes
/// rows from the library. `bib_save` is idempotent (matched identifiers
/// return as `cached` and cannot overwrite or replace) and `bib_add_note`
/// can only append annotations, so neither can be used to evict a
/// placeholder or mis-saved entry.
///
/// # Safety model
///
/// Deletion cascades through every FK constraint on the `articles` table:
/// `authors`, `identifiers`, `annotations`, `collection_articles`
/// memberships, and `fulltexts` pointer rows are removed automatically.
/// Stored full-text *files* on disk are NOT touched — the caller's
/// responsibility.
///
/// Pass `confirm=true` to actually delete. The default `confirm=false`
/// runs a **dry-run**: the same response shape is returned (with
/// `deleted=false` on every row) and the database is untouched. Use the
/// dry-run to preview what would be removed before committing.
///
/// # Input modes (at least one must be non-empty)
///
/// 1. **`article_ids`** — internal IDs (from `bib_search_library` or
///    `bib_get_article`). Direct delete.
/// 2. **`ids`** — typed external identifiers (`{ id_type, id }`).
///    Each is resolved via `find_by_identifier` to its internal id
///    before deletion; unresolved identifiers are reported as
///    `resolve_errors` rather than failing the whole batch.
#[tool(
    name = "bib_delete",
    description = "Delete one or more articles from the local bibliography library. \
                  This is the only path that physically removes an article: `bib_save` \
                  cannot overwrite or replace an existing row (matched identifiers are \
                  returned as `cached`), and `bib_add_note` can only append annotations. \
                  Use this tool to clean up placeholder rows, mis-saved entries (e.g. \
                  wrong PMID recall), or any article you no longer want in the library. \
                  Two input modes (at least one must be non-empty): (1) `article_ids` -- \
                  internal IDs from `bib_search_library` or `bib_get_article` (direct delete). \
                  (2) `ids` -- typed external identifiers `{ id_type, id }` resolved via \
                  `find_by_identifier` to internal ID; unresolved entries are returned in \
                  `resolve_errors` without failing the batch. Deletion cascades through FK \
                  constraints: `authors`, `identifiers`, `annotations`, `collection_articles` \
                  memberships, and `fulltexts` pointer rows are removed automatically. \
                  Stored full-text files on disk are NOT touched. Safety: pass \
                  `confirm=true` to actually delete; the default `confirm=false` runs as a \
                  dry-run that returns the same response shape without modifying the database."
)]
pub struct BibDeleteInput {
    #[desc = "Internal article IDs to delete. Discover via `bib_search_library` or \n             `bib_get_article`. Each id is deduplicated and processed at most once."]
    pub article_ids: Option<Vec<String>>,

    #[desc = "Typed external identifiers (`doi` / `pmid` / `arxiv` / `openalex` / \n             `s2` / `biorxiv`) to resolve and delete. Each entry is \n             `{ id_type, id }`. Resolution misses are reported in `resolve_errors` \n             and do not fail the rest of the batch."]
    pub ids: Option<Vec<ArticleIdInput>>,

    #[desc = "Set to `true` to actually delete. Default `false` runs a dry-run that \n             returns the same response shape without modifying the database."]
    pub confirm: Option<bool>,
}

pub struct BibDeleteTool {
    pub bib: Arc<BibBase>,
}

#[async_trait]
impl ToolFunction for BibDeleteTool {
    type Input = BibDeleteInput;

    async fn run(&self, input: Self::Input) -> Result<AgentToolResult, ToolError> {
        let has_internal = input.article_ids.as_ref().is_some_and(|v| !v.is_empty());
        let has_external = input.ids.as_ref().is_some_and(|v| !v.is_empty());

        if !has_internal && !has_external {
            return Err(ToolError::ExecutionFailed {
                source: "provide at least one of `article_ids` (internal IDs) or                          `ids` (typed external identifiers to resolve)"
                    .into(),
            });
        }

        let confirm = input.confirm.unwrap_or(false);

        // Resolve external identifiers (doi / pmid / ...) to internal ids.
        let mut resolved: Vec<String> = Vec::new();
        let mut resolve_errors: Vec<String> = Vec::new();

        if let Some(ext) = &input.ids {
            for entry in ext {
                let kind = match parse_id_kind(&entry.id_type) {
                    Some(k) => k,
                    None => {
                        resolve_errors.push(format!(
                            "unknown id_type '{}' for id '{}' (expected one of                              doi / pmid / arxiv / openalex / s2 / biorxiv)",
                            entry.id_type, entry.id
                        ));
                        continue;
                    }
                };
                match self
                    .bib
                    .find_by_identifier(kind, entry.id.trim())
                    .await
                    .map_err(box_error)?
                {
                    Some(art) => resolved.push(art.id),
                    None => resolve_errors.push(format!(
                        "no article found for {} '{}' (skipped)",
                        entry.id_type.to_uppercase(),
                        entry.id.trim()
                    )),
                }
            }
        }

        if let Some(internal) = &input.article_ids {
            resolved.extend(internal.iter().cloned());
        }

        // Deduplicate while preserving call order.
        let mut seen = std::collections::HashSet::new();
        resolved.retain(|id| seen.insert(id.clone()));

        // Always preview (so dry-run gets titles + found-flag).
        let mut previews: Vec<(String, Option<String>)> = Vec::with_capacity(resolved.len());
        for id in &resolved {
            let title = self
                .bib
                .get_article(id)
                .await
                .map_err(box_error)?
                .map(|a| a.title);
            previews.push((id.clone(), title));
        }

        let mut results: Vec<DeleteResult> = Vec::with_capacity(previews.len());
        let mut deleted_count = 0usize;
        let mut failed = 0usize;

        if confirm {
            for (id, title) in &previews {
                let found = title.is_some();
                match self.bib.delete_article(id).await.map_err(box_error) {
                    Ok(n) if n > 0 => {
                        deleted_count += 1;
                        results.push(DeleteResult {
                            article_id: id.clone(),
                            deleted: true,
                            found: true,
                            title: title.clone(),
                            error: None,
                        });
                    }
                    Ok(_) => {
                        // Row vanished between preview and delete (race) or
                        // the id never existed.
                        failed += 1;
                        results.push(DeleteResult {
                            article_id: id.clone(),
                            deleted: false,
                            found,
                            title: title.clone(),
                            error: Some(if found {
                                "row vanished between preview and delete".into()
                            } else {
                                "id not found".into()
                            }),
                        });
                    }
                    Err(e) => {
                        failed += 1;
                        results.push(DeleteResult {
                            article_id: id.clone(),
                            deleted: false,
                            found,
                            title: title.clone(),
                            error: Some(e.to_string()),
                        });
                    }
                }
            }
        } else {
            // Dry-run: report what *would* happen.
            for (id, title) in &previews {
                let found = title.is_some();
                results.push(DeleteResult {
                    article_id: id.clone(),
                    deleted: false,
                    found,
                    title: title.clone(),
                    error: None,
                });
            }
        }

        Ok(AgentToolResult::success_json(serde_json::json!({
            "mode": if confirm { "delete" } else { "dry_run" },
            "requested": resolved.len(),
            "deleted": deleted_count,
            "failed": failed,
            "resolve_errors": resolve_errors,
            "results": results,
            "message": if confirm {
                format!(
                    "{deleted_count} deleted, {failed} failed out of {} requested.                      Cascade removed authors, identifiers, annotations, collection                      memberships, and full-text pointers (files on disk are untouched).",
                    resolved.len()
                )
            } else {
                format!(
                    "DRY-RUN: no changes made. {} article(s) matched the request                      and would be deleted. Re-call with `confirm=true` to commit.",
                    resolved.len()
                )
            },
        })))
    }
}

// ===========================================================================
// bib_export — export articles in citation format
// ===========================================================================

#[tool(
    name = "bib_export",
    description = "Export articles from a collection (or the entire library) in a standard \
                  citation format: BibTeX, RIS, Markdown, or CSL-JSON. \
                  Useful for generating reference lists for manuscripts or reports."
)]
pub struct BibExportInput {
    #[desc = "Collection ID to export. If omitted, exports entire library."]
    pub collection_id: Option<String>,
    #[desc = "Output format: \"bibtex\", \"ris\", \"markdown\", or \"csl_json\". Default: bibtex"]
    pub format: Option<String>,
    #[desc = "Maximum articles to export (default 100)"]
    pub limit: Option<usize>,
}

pub struct BibExportTool {
    pub bib: Arc<BibBase>,
}

#[async_trait]
impl ToolFunction for BibExportTool {
    type Input = BibExportInput;

    async fn run(&self, input: Self::Input) -> Result<AgentToolResult, ToolError> {
        let format = parse_export_format(input.format.as_deref());
        let limit = input.limit.unwrap_or(100).clamp(1, 500);

        let articles: Vec<bib_types::Article> = match &input.collection_id {
            Some(cid) => {
                let cas = self
                    .bib
                    .list_collection_articles(cid, None, None)
                    .await
                    .map_err(box_error)?;
                let mut out = Vec::new();
                for ca in cas.into_iter().take(limit) {
                    if let Some(a) = self
                        .bib
                        .get_article(&ca.article_id)
                        .await
                        .map_err(box_error)?
                    {
                        out.push(a);
                    }
                }
                out
            }
            None => {
                let hits = self
                    .bib
                    .search_articles("", limit)
                    .await
                    .map_err(box_error)?;
                let mut out = Vec::new();
                for hit in hits {
                    if let Some(a) = self
                        .bib
                        .get_article(&hit.article_id)
                        .await
                        .map_err(box_error)?
                    {
                        out.push(a);
                    }
                }
                out
            }
        };

        let rendered = crate::export::render_all(&articles, format);
        let count = articles.len();

        Ok(AgentToolResult::success_json(serde_json::json!({
            "format": format_extension(format),
            "count": count,
            "export": rendered,
        })))
    }
}

// ===========================================================================
// Registration
// ===========================================================================

/// Build [`ToolRegistration`]s for all library management tools.
///
/// Requires a [`BibBase`] (for storage), a [`LiteratureGateway`] (for
/// `bib_save` external fetching), and a shared [`EuropePmcClient`] (for
/// the OA full-text auto-fetch inside `bib_save`).
///
/// `epmc` should normally come from [`crate::BibShared::europe_pmc`] so
/// every agent in a multi-agent host shares a single connection pool.
/// A fresh `EuropePmcClient` is constructed only if `None` is passed —
/// kept for backwards compatibility and standalone single-agent use.
pub fn bib_library_registrations(
    bib: Arc<BibBase>,
    gateway: Arc<LiteratureGateway>,
    epmc: Option<Arc<EuropePmcClient>>,
) -> Vec<ToolRegistration> {
    use agentik_core::tools::ToolRegistration as R;
    let epmc = epmc.unwrap_or_else(|| Arc::new(EuropePmcClient::new()));
    vec![
        R::from(BibSaveTool {
            bib: bib.clone(),
            gateway: gateway.clone(),
            epmc,
            easyscholar: Arc::new(easyscholar::EasyscholarClient::from_env()),
        }),
        R::from(BibCreateCollectionTool { bib: bib.clone() }),
        R::from(BibAddToCollectionTool { bib: bib.clone() }),
        R::from(BibListCollectionTool { bib: bib.clone() }),
        R::from(BibSearchLibraryTool { bib: bib.clone() }),
        R::from(BibGetArticleTool { bib: bib.clone() }),
        R::from(BibRequestFulltextTool { bib: bib.clone() }),
        R::from(BibAddNoteTool { bib: bib.clone() }),
        R::from(BibDeleteTool { bib: bib.clone() }),
        R::from(BibExportTool { bib }),
    ]
}

/// Build [`ToolRegistration`]s for **all** bibliography tools — both query
/// and management. This is the one-stop registration function.
///
/// When called from a multi-agent host, pass the shared
/// [`EuropePmcClient`] from [`crate::BibShared::europe_pmc`] so every
/// spawned agent reuses the same connection pool. Pass `None` to
/// allocate a fresh client (single-agent / test use).
///
/// # Example
///
/// ```no_run
/// # use std::sync::Arc;
/// # use bib_base::{BibBase, BibShared, bib_all_registrations};
/// # async fn example() {
/// let shared = BibShared::open_in_memory().await.unwrap();
/// let tools = bib_all_registrations(
///     shared.bib.clone(),
///     shared.gateway.clone(),
///     Some(shared.europe_pmc.clone()),
/// );
/// # }
/// ```
pub fn bib_all_registrations(
    bib: Arc<BibBase>,
    gateway: Arc<LiteratureGateway>,
    epmc: Option<Arc<europepmc::EuropePmcClient>>,
) -> Vec<ToolRegistration> {
    let mut tools = crate::tools::bib_query_registrations(gateway.clone());
    tools.extend(bib_library_registrations(bib, gateway, epmc));
    tools
}

/// Build [`ToolRegistration`]s for the extended literature tools whose
/// capabilities are **not** covered by the [`LiteratureGateway`].
///
/// When [`BibShared`](crate::BibShared) is constructed, three additional
/// sources (OpenAlex, Crossref, Semantic Scholar) are loaded into the
/// gateway for unified `search`/`fetch`. Each of these APIs, however, also
/// offers source-specific features that fall outside the
/// [`LiteratureSource`](crate::LiteratureSource) contract:
///
/// | Source            | Extended tools                                   |
/// |-------------------|--------------------------------------------------|
/// | OpenAlex          | `openalex_autocomplete` (cross-entity typeahead) |
/// | Crossref          | `crossref_types` (work-type catalogue)           |
/// | Semantic Scholar  | `s2_citations`, `s2_references`,                 |
/// |                   | `s2_recommendations`, `s2_author`                |
///
/// Pass the shared clients from [`BibShared`](crate::BibShared) so every
/// agent reuses the same connection pools.
pub fn bib_extended_registrations(
    openalex_client: Arc<openalex::OpenAlexClient>,
    crossref_client: Arc<crossref::CrossrefClient>,
    s2_client: Arc<semantic_scholar::S2Client>,
) -> Vec<ToolRegistration> {
    let mut tools = Vec::new();
    tools.extend(openalex::openalex_extended_registrations(openalex_client));
    tools.extend(crossref::crossref_extended_registrations(crossref_client));
    tools.extend(semantic_scholar::s2_extended_registrations(s2_client));
    tools
}

// ===========================================================================
// Helpers
// ===========================================================================

/// Parse a role string, defaulting to [`ArticleRole::Referenced`].
fn parse_role(s: Option<&str>) -> ArticleRole {
    match s.map(|x| x.to_lowercase()).as_deref() {
        Some("requested") => ArticleRole::Requested,
        Some("cited") => ArticleRole::Cited,
        Some("background") => ArticleRole::Background,
        _ => ArticleRole::Referenced,
    }
}

/// Parse an optional role string into `Some(ArticleRole)` or `None`.
fn parse_role_opt(s: &str) -> Option<ArticleRole> {
    match s.to_lowercase().as_str() {
        "requested" => Some(ArticleRole::Requested),
        "referenced" => Some(ArticleRole::Referenced),
        "cited" => Some(ArticleRole::Cited),
        "background" => Some(ArticleRole::Background),
        _ => None,
    }
}

/// Parse an optional fetch_status string.
fn parse_fetch_status_opt(s: &str) -> Option<FetchStatus> {
    match s {
        "metadata_only" => Some(FetchStatus::MetadataOnly),
        "fulltext_requested" => Some(FetchStatus::FulltextRequested),
        "fulltext_available" => Some(FetchStatus::FulltextAvailable),
        _ => None,
    }
}

/// Wrap a bib-base [`crate::Error`] into a [`ToolError`].
fn box_error<E: std::error::Error + Send + Sync + 'static>(e: E) -> ToolError {
    ToolError::ExecutionFailed {
        source: Box::new(e),
    }
}

/// Parse export format string, defaulting to BibTeX.
fn parse_export_format(s: Option<&str>) -> bib_types::ExportFormat {
    match s.map(|x| x.to_lowercase()).as_deref() {
        Some("ris") => bib_types::ExportFormat::Ris,
        Some("markdown") | Some("md") => bib_types::ExportFormat::Markdown,
        Some("csl_json") | Some("csljson") | Some("json") => bib_types::ExportFormat::CslJson,
        _ => bib_types::ExportFormat::Bibtex,
    }
}

/// File extension for an export format.
fn format_extension(format: bib_types::ExportFormat) -> &'static str {
    format.extension()
}

// ===========================================================================
// Unit tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use agentik_sdk::types::ToolResultContent;
    use bib_types::IdKind;

    // ── article_input_to_article ──────────────────────────────────────────

    #[test]
    fn test_article_input_with_doi_only() {
        let input = ArticleInput {
            title: "GWAS of height".into(),
            doi: Some("10.1038/ng.1234".into()),
            pmid: None,
            identifiers: vec![],
            authors: vec!["Smith John".into(), "Doe Kate".into()],
            year: Some(2024),
            journal: Some("Nature Genetics".into()),
            volume: None,
            issue: None,
            pages: None,
            abstract_text: Some("A study of height.".into()),
            keywords: vec!["GWAS".into()],
            pub_types: vec!["Journal Article".into()],
            source: Some("pubmed".into()),
        };

        let article = article_input_to_article(&input).unwrap();
        assert_eq!(article.title, "GWAS of height");
        assert_eq!(article.doi(), Some("10.1038/ng.1234"));
        assert_eq!(article.authors.len(), 2);
        assert_eq!(article.authors[0].last_name, "Smith");
        assert_eq!(article.authors[0].fore_name.as_deref(), Some("John"));
        assert_eq!(article.authors[1].last_name, "Doe");
        assert_eq!(article.year, Some(2024));
        assert_eq!(article.journal.as_deref(), Some("Nature Genetics"));
        assert_eq!(article.source, bib_types::ArticleSource::Pubmed);
        assert_eq!(article.id, "doi:10.1038/ng.1234");
    }

    #[test]
    fn test_article_input_with_structured_identifiers() {
        let input = ArticleInput {
            title: "Deep learning for genomics".into(),
            doi: None,
            pmid: None,
            identifiers: vec![
                IdentifierInput {
                    kind: "arxiv".into(),
                    value: "2401.00001".into(),
                },
                IdentifierInput {
                    kind: "doi".into(),
                    value: "10.48550/arXiv.2401.00001".into(),
                },
            ],
            authors: vec![],
            year: Some(2024),
            journal: None,
            volume: None,
            issue: None,
            pages: None,
            abstract_text: None,
            keywords: vec![],
            pub_types: vec![],
            source: Some("arxiv".into()),
        };

        let article = article_input_to_article(&input).unwrap();
        assert_eq!(article.identifiers.len(), 2);
        assert_eq!(article.identifier(IdKind::Arxiv), Some("2401.00001"));
        assert_eq!(article.source, bib_types::ArticleSource::Arxiv);
    }

    #[test]
    fn test_article_input_dedup_identifiers() {
        // DOI provided both as top-level field and in identifiers array.
        let input = ArticleInput {
            title: "Test".into(),
            doi: Some("10.1038/test".into()),
            pmid: Some("12345".into()),
            identifiers: vec![IdentifierInput {
                kind: "doi".into(),
                value: "10.1038/test".into(),
            }],
            authors: vec![],
            year: None,
            journal: None,
            volume: None,
            issue: None,
            pages: None,
            abstract_text: None,
            keywords: vec![],
            pub_types: vec![],
            source: None,
        };

        let article = article_input_to_article(&input).unwrap();
        // DOI should appear only once.
        let doi_count = article
            .identifiers
            .iter()
            .filter(|i| i.kind == IdKind::Doi)
            .count();
        assert_eq!(doi_count, 1);
        // PMID should also be present.
        assert_eq!(article.pmid(), Some("12345"));
    }

    #[test]
    fn test_article_input_missing_title() {
        let input = ArticleInput {
            title: "  ".into(),
            doi: Some("10.1038/x".into()),
            pmid: None,
            identifiers: vec![],
            authors: vec![],
            year: None,
            journal: None,
            volume: None,
            issue: None,
            pages: None,
            abstract_text: None,
            keywords: vec![],
            pub_types: vec![],
            source: None,
        };

        assert!(article_input_to_article(&input).is_err());
    }

    #[test]
    fn test_article_input_missing_identifiers() {
        let input = ArticleInput {
            title: "No IDs".into(),
            doi: None,
            pmid: None,
            identifiers: vec![],
            authors: vec![],
            year: None,
            journal: None,
            volume: None,
            issue: None,
            pages: None,
            abstract_text: None,
            keywords: vec![],
            pub_types: vec![],
            source: None,
        };

        assert!(article_input_to_article(&input).is_err());
    }

    #[test]
    fn test_article_input_single_name_author() {
        let input = ArticleInput {
            title: "Single".into(),
            doi: Some("10.1/x".into()),
            pmid: None,
            identifiers: vec![],
            authors: vec!["Organization".into()],
            year: None,
            journal: None,
            volume: None,
            issue: None,
            pages: None,
            abstract_text: None,
            keywords: vec![],
            pub_types: vec![],
            source: None,
        };

        let article = article_input_to_article(&input).unwrap();
        assert_eq!(article.authors.len(), 1);
        assert_eq!(article.authors[0].last_name, "Organization");
        assert!(article.authors[0].fore_name.is_none());
    }

    // ── Direct save (no external fetch) ───────────────────────────────────

    #[tokio::test]
    async fn test_save_article_direct_basic() {
        let bib = Arc::new(BibBase::open_in_memory().await.unwrap());
        let gateway = Arc::new(crate::default_gateway());
        let epmc = Arc::new(EuropePmcClient::new());
        let get_tool = BibGetArticleTool { bib: bib.clone() };
        let tool = BibSaveTool {
            bib: bib.clone(),
            gateway,
            epmc,
            easyscholar: Arc::new(easyscholar::EasyscholarClient::new(None)),
        };

        let input = BibSaveInput {
            articles: Some(vec![ArticleInput {
                title: "Mendelian randomization study".into(),
                doi: Some("10.1038/ng.2024.001".into()),
                pmid: Some("39000001".into()),
                identifiers: vec![],
                authors: vec!["Smith Jane".into(), "Roe Richard".into()],
                year: Some(2024),
                journal: Some("Nature Genetics".into()),
                volume: Some("56".into()),
                issue: None,
                pages: Some("100-110".into()),
                abstract_text: Some("We investigate causal effects.".into()),
                keywords: vec!["MR".into(), "genetics".into()],
                pub_types: vec!["Journal Article".into()],
                source: Some("pubmed".into()),
            }]),
            ids: None,
            source: None,
            fetch_fulltext: Some(false),
        };

        let result = tool.run(input).await.unwrap();
        let json = match result.content {
            ToolResultContent::Json(v) => v,
            _ => panic!("expected JSON"),
        };

        assert_eq!(json["total"], 1);
        assert_eq!(json["saved"].as_u64(), Some(1));
        assert_eq!(json["cached"].as_u64(), Some(0));

        let r = &json["results"][0];
        assert_eq!(r["saved"], true);
        assert_eq!(r["cached"], false);
        assert_eq!(r["doi"], "10.1038/ng.2024.001");
        assert_eq!(r["pmid"], "39000001");
        assert_eq!(r["year"], 2024);
        assert!(r["article_id"].as_str().is_some());

        let article_id = r["article_id"].as_str().unwrap().to_owned();
        let text = "0123456789".repeat(10);
        bib.upsert_fulltext(&bib_types::FullText {
            article_id: article_id.clone(),
            file_path: "vfs:///literature/test/source.txt".into(),
            file_format: bib_types::FileFormat::Txt,
            text_content: Some(text),
            source: bib_types::FullTextSource::UserUpload,
            file_hash: None,
            file_size: None,
            uploaded_at: None,
            parse_status: "done".to_owned(),
            parse_engine: None,
            parse_error: None,
        })
        .await
        .unwrap();

        let default_result = get_tool
            .run(BibGetArticleInput {
                article_ids: vec![article_id.clone()],
                include_fulltext: None,
                offset: None,
                limit: None,
            })
            .await
            .unwrap();
        let default_json = match default_result.content {
            ToolResultContent::Json(value) => value,
            _ => panic!("expected JSON"),
        };
        assert_eq!(default_json["results"][0]["has_fulltext"], true);
        assert!(default_json["results"][0].get("fulltext").is_none());

        let paged_result = get_tool
            .run(BibGetArticleInput {
                article_ids: vec![article_id],
                include_fulltext: Some(true),
                offset: Some(20),
                limit: Some(7),
            })
            .await
            .unwrap();
        let paged_json = match paged_result.content {
            ToolResultContent::Json(value) => value,
            _ => panic!("expected JSON"),
        };
        let paged = &paged_json["results"][0];
        assert_eq!(paged["fulltext"], "0123456");
        assert_eq!(paged["fulltext_offset"], 20);
        assert_eq!(paged["fulltext_limit"], 7);
        assert_eq!(paged["fulltext_total_chars"], 100);
        assert_eq!(paged["fulltext_next_offset"], 27);
    }

    #[tokio::test]
    async fn test_save_article_direct_cached() {
        let bib = Arc::new(BibBase::open_in_memory().await.unwrap());
        let gateway = Arc::new(crate::default_gateway());
        let epmc = Arc::new(EuropePmcClient::new());
        let tool = BibSaveTool { bib, gateway, epmc, easyscholar: Arc::new(easyscholar::EasyscholarClient::new(None)) };

        let article = ArticleInput {
            title: "Cached test".into(),
            doi: Some("10.1038/cached.001".into()),
            pmid: None,
            identifiers: vec![],
            authors: vec![],
            year: Some(2023),
            journal: None,
            volume: None,
            issue: None,
            pages: None,
            abstract_text: None,
            keywords: vec![],
            pub_types: vec![],
            source: None,
        };

        // First save.
        let input1 = BibSaveInput {
            articles: Some(vec![article.clone()]),
            ids: None,
            source: None,
            fetch_fulltext: Some(false),
        };
        let _ = tool.run(input1).await.unwrap();

        // Second save — should be cached.
        let input2 = BibSaveInput {
            articles: Some(vec![article]),
            ids: None,
            source: None,
            fetch_fulltext: Some(false),
        };
        let result2 = tool.run(input2).await.unwrap();
        let json2 = match result2.content {
            ToolResultContent::Json(v) => v,
            _ => panic!("expected JSON"),
        };

        assert_eq!(json2["saved"].as_u64(), Some(0));
        assert_eq!(json2["cached"].as_u64(), Some(1));
    }

    #[tokio::test]
    async fn test_save_article_direct_batch() {
        let bib = Arc::new(BibBase::open_in_memory().await.unwrap());
        let gateway = Arc::new(crate::default_gateway());
        let epmc = Arc::new(EuropePmcClient::new());
        let tool = BibSaveTool { bib, gateway, epmc, easyscholar: Arc::new(easyscholar::EasyscholarClient::new(None)) };

        let input = BibSaveInput {
            articles: Some(vec![
                ArticleInput {
                    title: "Article A".into(),
                    doi: Some("10.1/a".into()),
                    pmid: None,
                    identifiers: vec![],
                    authors: vec![],
                    year: Some(2020),
                    journal: None,
                    volume: None,
                    issue: None,
                    pages: None,
                    abstract_text: None,
                    keywords: vec![],
                    pub_types: vec![],
                    source: None,
                },
                ArticleInput {
                    title: "Article B".into(),
                    doi: None,
                    pmid: Some("99999".into()),
                    identifiers: vec![],
                    authors: vec![],
                    year: Some(2021),
                    journal: None,
                    volume: None,
                    issue: None,
                    pages: None,
                    abstract_text: None,
                    keywords: vec![],
                    pub_types: vec![],
                    source: None,
                },
                ArticleInput {
                    title: "Article C (arXiv)".into(),
                    doi: None,
                    pmid: None,
                    identifiers: vec![IdentifierInput {
                        kind: "arxiv".into(),
                        value: "2101.00001".into(),
                    }],
                    authors: vec![],
                    year: Some(2021),
                    journal: None,
                    volume: None,
                    issue: None,
                    pages: None,
                    abstract_text: None,
                    keywords: vec![],
                    pub_types: vec![],
                    source: Some("arxiv".into()),
                },
            ]),
            ids: None,
            source: None,
            fetch_fulltext: Some(false),
        };

        let result = tool.run(input).await.unwrap();
        let json = match result.content {
            ToolResultContent::Json(v) => v,
            _ => panic!("expected JSON"),
        };

        assert_eq!(json["total"], 3);
        assert_eq!(json["saved"].as_u64(), Some(3));
        assert_eq!(json["failed"].as_u64(), Some(0));

        let results = json["results"].as_array().unwrap();
        assert_eq!(results.len(), 3);
        for r in results {
            assert_eq!(r["saved"], true);
            assert!(r["article_id"].as_str().is_some());
        }
    }

    #[tokio::test]
    async fn test_save_rejects_empty_input() {
        let bib = Arc::new(BibBase::open_in_memory().await.unwrap());
        let gateway = Arc::new(crate::default_gateway());
        let epmc = Arc::new(EuropePmcClient::new());
        let tool = BibSaveTool { bib, gateway, epmc, easyscholar: Arc::new(easyscholar::EasyscholarClient::new(None)) };

        let input = BibSaveInput {
            articles: None,
            ids: None,
            source: None,
            fetch_fulltext: None,
        };

        assert!(tool.run(input).await.is_err());
    }

    #[tokio::test]
    async fn test_save_article_direct_error_no_id() {
        let bib = Arc::new(BibBase::open_in_memory().await.unwrap());
        let gateway = Arc::new(crate::default_gateway());
        let epmc = Arc::new(EuropePmcClient::new());
        let tool = BibSaveTool { bib, gateway, epmc, easyscholar: Arc::new(easyscholar::EasyscholarClient::new(None)) };

        let input = BibSaveInput {
            articles: Some(vec![ArticleInput {
                title: "No identifiers".into(),
                doi: None,
                pmid: None,
                identifiers: vec![],
                authors: vec![],
                year: None,
                journal: None,
                volume: None,
                issue: None,
                pages: None,
                abstract_text: None,
                keywords: vec![],
                pub_types: vec![],
                source: None,
            }]),
            ids: None,
            source: None,
            fetch_fulltext: Some(false),
        };

        let result = tool.run(input).await.unwrap();
        let json = match result.content {
            ToolResultContent::Json(v) => v,
            _ => panic!("expected JSON"),
        };

        assert_eq!(json["failed"].as_u64(), Some(1));
        assert!(json["results"][0]["error"].as_str().is_some());
    }

    // ── bib_delete ────────────────────────────────────────────────────────

    /// Helper: seed an in-memory `BibBase` with one article that has a
    /// DOI and a PMID identifier, returning its internal id.
    async fn seed_one_article() -> (Arc<BibBase>, String) {
        let bib = Arc::new(BibBase::open_in_memory().await.unwrap());
        let mut art = bib_types::Article::new("doi:10.1038/ng.2024.999", "Test paper");
        art.identifiers
            .push(bib_types::Identifier::doi("10.1038/ng.2024.999"));
        art.identifiers
            .push(bib_types::Identifier::pmid("39000999"));
        art.year = Some(2024);
        bib.upsert_article(&art).await.unwrap();
        (bib, art.id)
    }

    #[tokio::test]
    async fn test_delete_by_article_id() {
        let (bib, id) = seed_one_article().await;
        assert_eq!(bib.article_count().await.unwrap(), 1);

        let tool = BibDeleteTool { bib: bib.clone() };
        let input = BibDeleteInput {
            article_ids: Some(vec![id.clone()]),
            ids: None,
            confirm: Some(true),
        };

        let result = tool.run(input).await.unwrap();
        let json = match result.content {
            ToolResultContent::Json(v) => v,
            _ => panic!("expected JSON"),
        };

        assert_eq!(json["mode"], "delete");
        assert_eq!(json["requested"].as_u64(), Some(1));
        assert_eq!(json["deleted"].as_u64(), Some(1));
        assert_eq!(json["failed"].as_u64(), Some(0));
        assert_eq!(json["results"][0]["article_id"], id);
        assert_eq!(json["results"][0]["deleted"], true);
        assert_eq!(json["results"][0]["found"], true);
        assert_eq!(bib.article_count().await.unwrap(), 0);
    }

    #[tokio::test]
    async fn test_delete_dry_run_is_noop() {
        let (bib, id) = seed_one_article().await;
        let tool = BibDeleteTool { bib: bib.clone() };

        // confirm omitted (defaults to false).
        let input = BibDeleteInput {
            article_ids: Some(vec![id.clone()]),
            ids: None,
            confirm: None,
        };
        let result = tool.run(input).await.unwrap();
        let json = match result.content {
            ToolResultContent::Json(v) => v,
            _ => panic!("expected JSON"),
        };

        assert_eq!(json["mode"], "dry_run");
        assert_eq!(json["requested"].as_u64(), Some(1));
        assert_eq!(json["deleted"].as_u64(), Some(0));
        assert_eq!(json["results"][0]["deleted"], false);
        assert_eq!(json["results"][0]["found"], true);
        // Article must still exist.
        assert_eq!(bib.article_count().await.unwrap(), 1);
    }

    #[tokio::test]
    async fn test_delete_by_doi_resolves_internal_id() {
        let (bib, _id) = seed_one_article().await;
        let tool = BibDeleteTool { bib: bib.clone() };

        let input = BibDeleteInput {
            article_ids: None,
            ids: Some(vec![ArticleIdInput {
                id_type: "doi".into(),
                id: "10.1038/ng.2024.999".into(),
            }]),
            confirm: Some(true),
        };

        let result = tool.run(input).await.unwrap();
        let json = match result.content {
            ToolResultContent::Json(v) => v,
            _ => panic!("expected JSON"),
        };

        assert_eq!(json["deleted"].as_u64(), Some(1));
        assert_eq!(bib.article_count().await.unwrap(), 0);
    }

    #[tokio::test]
    async fn test_delete_unresolvable_doi_is_reported_not_failed() {
        let (bib, _id) = seed_one_article().await;
        let tool = BibDeleteTool { bib: bib.clone() };

        let input = BibDeleteInput {
            article_ids: None,
            ids: Some(vec![ArticleIdInput {
                id_type: "doi".into(),
                id: "10.1038/does-not-exist".into(),
            }]),
            confirm: Some(true),
        };
        let result = tool.run(input).await.unwrap();
        let json = match result.content {
            ToolResultContent::Json(v) => v,
            _ => panic!("expected JSON"),
        };

        assert_eq!(json["requested"].as_u64(), Some(0));
        assert_eq!(json["deleted"].as_u64(), Some(0));
        assert!(
            json["resolve_errors"][0]
                .as_str()
                .unwrap()
                .contains("no article found")
        );
        // Original article must still exist.
        assert_eq!(bib.article_count().await.unwrap(), 1);
    }

    #[tokio::test]
    async fn test_delete_missing_internal_id_reports_id_not_found() {
        let bib = Arc::new(BibBase::open_in_memory().await.unwrap());
        let tool = BibDeleteTool { bib: bib.clone() };

        let input = BibDeleteInput {
            article_ids: Some(vec!["never-existed".into()]),
            ids: None,
            confirm: Some(true),
        };
        let result = tool.run(input).await.unwrap();
        let json = match result.content {
            ToolResultContent::Json(v) => v,
            _ => panic!("expected JSON"),
        };

        assert_eq!(json["requested"].as_u64(), Some(1));
        assert_eq!(json["deleted"].as_u64(), Some(0));
        assert_eq!(json["failed"].as_u64(), Some(1));
        assert_eq!(json["results"][0]["found"], false);
        assert_eq!(json["results"][0]["error"].as_str(), Some("id not found"));
    }

    #[tokio::test]
    async fn test_delete_rejects_empty_input() {
        let bib = Arc::new(BibBase::open_in_memory().await.unwrap());
        let tool = BibDeleteTool { bib };
        let input = BibDeleteInput {
            article_ids: None,
            ids: None,
            confirm: Some(true),
        };
        let err = tool.run(input).await.unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("provide at least one"),
            "unexpected error: {msg}"
        );
    }

    #[tokio::test]
    async fn test_delete_cascade_clears_annotations_and_collections() {
        let bib = Arc::new(BibBase::open_in_memory().await.unwrap());

        // Seed an article, an annotation, and a collection membership.
        let mut art = bib_types::Article::new("doi:10.1/cascade", "Cascade target");
        art.identifiers
            .push(bib_types::Identifier::doi("10.1/cascade"));
        bib.upsert_article(&art).await.unwrap();
        bib.add_annotation(&art.id, bib_types::AnnotationKind::Note, "stale note", None, None)
            .await
            .unwrap();
        let col = bib_types::Collection::new("col-junk", "junk-collection");
        bib.upsert_collection(&col).await.unwrap();
        bib.add_to_collection(
            &col.id,
            &art.id,
            ArticleRole::Referenced,
            AddedBy::Agent,
            None,
        )
        .await
        .unwrap();

        // Sanity: annotation + membership present.
        assert_eq!(bib.list_annotations(&art.id).await.unwrap().len(), 1);

        let tool = BibDeleteTool { bib: bib.clone() };
        let input = BibDeleteInput {
            article_ids: Some(vec![art.id.clone()]),
            ids: None,
            confirm: Some(true),
        };
        tool.run(input).await.unwrap();

        // FK CASCADE should have wiped everything.
        assert_eq!(bib.article_count().await.unwrap(), 0);
        assert_eq!(bib.list_annotations(&art.id).await.unwrap().len(), 0);
        assert_eq!(
            bib.list_collection_articles(&col.id, None, None)
                .await
                .unwrap()
                .len(),
            0
        );
    }
}
