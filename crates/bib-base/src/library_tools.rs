//! Library management agent tools — persist, organize, and retrieve
//! articles from the local Turso-backed bibliography database.
//!
//! These tools complement the external query tools ([`lit_search`],
//! [`lit_fetch`]) by bridging "found on the internet" → "stored locally
//! and organized into collections".
//!
//! # Tool inventory
//!
//! | Tool                   | Purpose                                         |
//! |------------------------|--------------------------------------------------|
//! | `bib_save`             | Fetch + store an article in the local library.   |
//! | `bib_create_collection`| Create a new collection.                         |
//! | `bib_add_to_collection`| Add one or more articles to a collection + roles. |
//! | `bib_list_collection`  | List collections or articles within one.         |
//! | `bib_search_library`   | LIKE search across local library (multi-query).  |
//! | `bib_get_article`      | Batch fetch metadata + full text (concurrent).   |
//! | `bib_request_fulltext` | Mark an article as needing full-text upload.     |

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
// bib_save — fetch from external source and store in local library (batch)
// ===========================================================================

/// A single typed article identifier for `bib_save`.
#[derive(schemars::JsonSchema, serde::Deserialize, serde::Serialize)]
pub struct ArticleIdInput {
    #[schemars(description = "Identifier type: \"doi\", \"pmid\", \"arxiv\", \"openalex\", \"s2\", or \"biorxiv\"")]
    pub id_type: String,
    #[schemars(description = "The identifier value (e.g. \"10.1038/...\", \"30124452\", \"W2741809807\")")]
    pub id: String,
}

#[tool(
    name = "bib_save",
    description = "Save one or more articles to the local library by fetching their \
                  metadata from external sources (PubMed, arXiv, OpenAlex, Crossref, \
                  Semantic Scholar, bioRxiv). \
                  \
                  Each article is identified by a typed `{ id_type, id }` pair so the \
                  gateway knows exactly which sources to query — no format guessing. \
                  Articles are fetched concurrently and stored. \
                  \
                  Articles already in the library (matched by identifier) are \
                  returned as cached without re-fetching. \
                  \
                  After saving the metadata, each newly stored article is \
                  **automatically checked for an open-access full text** on \
                  Europe PMC. When a full text is available it is downloaded \
                  (JATS XML → plain text) and stored alongside the article, so \
                  `bib_get_article` can return it immediately. Set \
                  `fetch_fulltext=false` to skip this step. \
                  \
                  **Examples**: \
                  • ids=[{id_type:\"pmid\", id:\"37658030\"}] — save a single PMID \
                  • ids=[{id_type:\"doi\", id:\"10.1038/...\"}, {id_type:\"arxiv\", id:\"2401.00001\"}] — batch"
)]
pub struct BibSaveInput {
    #[desc = "One or more typed article identifiers (id_type + id). Use the same identifiers \
             you found in lit_search/lit_fetch results."]
    pub ids: Vec<ArticleIdInput>,
    #[desc = "Force a specific source for ALL ids (e.g. \"pubmed\", \"crossref\"). \
             Default: auto-route each id to compatible sources."]
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
        if input.ids.is_empty() {
            return Err(ToolError::ExecutionFailed {
                source: "at least one article ID is required".into(),
            });
        }

        let want_fulltext = input.fetch_fulltext.unwrap_or(true);

        // Parse each typed id into an Identifier, collecting errors.
        let mut identifiers: Vec<(String, Identifier)> = Vec::new();
        for entry in &input.ids {
            let kind = parse_id_kind(&entry.id_type).ok_or_else(|| ToolError::ExecutionFailed {
                source: format!(
                    "unknown id_type '{}': expected one of doi, pmid, arxiv, openalex, s2, biorxiv",
                    entry.id_type
                )
                .into(),
            })?;
            identifiers.push((entry.id.clone(), Identifier::new(kind, entry.id.trim())));
        }

        // Process each identifier concurrently.
        let futures: Vec<_> = identifiers
            .iter()
            .map(|(raw, id)| self.save_one(raw, id, input.source.as_deref(), want_fulltext))
            .collect();
        let results = futures::future::join_all(futures).await;

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
                  If full text has not been uploaded, the `has_fulltext` field will be \
                  false and `fulltext` will be null. Set `include_fulltext=false` to \
                  skip the full-text column (faster, smaller responses). \
                  \
                  **Examples**: \
                  • article_ids=[\"a1\"] — fetch a single article \
                  • article_ids=[\"a1\", \"a2\", \"a3\"] — batch (fetched concurrently)"
)]
pub struct BibGetArticleInput {
    #[desc = "One or more article IDs (from bib_save or bib_search_library results)"]
    pub article_ids: Vec<String>,
    #[desc = "If true and full text is available, include the full text content in each result. Default: true"]
    pub include_fulltext: Option<bool>,
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

        let include_ft = input.include_fulltext.unwrap_or(true);

        // Fetch each article concurrently. The local DB is fast, but batching
        // still collapses N tool-call round-trips into one and lets the reads
        // overlap on the connection pool.
        let futures: Vec<_> = input
            .article_ids
            .iter()
            .map(|id| self.get_one(id.trim(), include_ft))
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
    async fn get_one(&self, article_id: &str, include_ft: bool) -> GetArticleResult {
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
        let has_fulltext = match self.bib.has_fulltext(article_id).await {
            Ok(b) => b,
            Err(e) => return GetArticleResult::err(article_id, false, e.to_string()),
        };

        let fulltext = if include_ft {
            match self.bib.get_fulltext(article_id).await {
                Ok(ft) => ft,
                Err(e) => return GetArticleResult::err(article_id, false, e.to_string()),
            }
        } else {
            None
        };
        let text_content = fulltext.as_ref().and_then(|ft| ft.text_content.clone());

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
            fulltext_source: fulltext
                .as_ref()
                .map(|ft| ft.source.as_str().to_owned()),
            fulltext: text_content,
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
                  \"fulltext_available\" and bib_get_article will return the full text."
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
            .add_annotation(&input.article_id, kind, &input.content, input.page)
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
        }),
        R::from(BibCreateCollectionTool { bib: bib.clone() }),
        R::from(BibAddToCollectionTool { bib: bib.clone() }),
        R::from(BibListCollectionTool { bib: bib.clone() }),
        R::from(BibSearchLibraryTool { bib: bib.clone() }),
        R::from(BibGetArticleTool { bib: bib.clone() }),
        R::from(BibRequestFulltextTool { bib: bib.clone() }),
        R::from(BibAddNoteTool { bib: bib.clone() }),
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
