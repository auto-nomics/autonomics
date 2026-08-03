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
//! | `bib_add_to_collection`| Associate an article with a collection + role.   |
//! | `bib_list_collection`  | List collections or articles within one.         |
//! | `bib_search_library`   | LIKE search across local library.                |
//! | `bib_get_article`      | Get full metadata + full text from local library.|
//! | `bib_request_fulltext` | Mark an article as needing full-text upload.     |

use std::sync::Arc;

use agentik_core::tools::{ToolError, ToolFunction, ToolRegistration};
use agentik_proc::tool;
use agentik_sdk::types::ToolResult as AgentToolResult;
use async_trait::async_trait;
use bib_types::{AddedBy, ArticleRole, CollectionStatus, FetchStatus, IdKind};

use crate::bib_base::BibBase;
use crate::query::LiteratureGateway;
use crate::tools::auto_fetch;

// ===========================================================================
// bib_save — fetch from external source and store in local library (batch)
// ===========================================================================

#[tool(
    name = "bib_save",
    description = "Save one or more articles to the local library by fetching their \
                  metadata from the appropriate external source (PubMed, arXiv). \
                  \
                  Pass a list of IDs — each is fetched concurrently and stored. \
                  Articles already in the library (matched by DOI or PMID) are \
                  returned as cached without re-fetching. \
                  \
                  Use this after `lit_search` or `lit_fetch` to persist articles you \
                  want to keep. Pass the same IDs you found in search results. \
                  \
                  **Examples**: \
                  • ids=[\"37658030\"] — save a single PMID \
                  • ids=[\"37658030\", \"10.1038/s41586-023-06236-2\", \"2401.00001\"] — batch"
)]
pub struct BibSaveInput {
    #[desc = "One or more article identifiers: PMIDs, DOIs, or arXiv IDs (same IDs you got from lit_search/lit_fetch)"]
    pub ids: Vec<String>,
    #[desc = "Force a specific source for ALL ids: \"pubmed\" or \"arxiv\". Default: auto-detect each ID from its format."]
    pub source: Option<String>,
}

pub struct BibSaveTool {
    pub bib: Arc<BibBase>,
    pub gateway: Arc<LiteratureGateway>,
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

        // Process each ID concurrently.
        let futures: Vec<_> = input
            .ids
            .iter()
            .map(|id| self.save_one(id.trim(), input.source.as_deref()))
            .collect();
        let results = futures::future::join_all(futures).await;

        let total = results.len();
        let saved = results.iter().filter(|r| r.saved && !r.cached).count();
        let cached = results.iter().filter(|r| r.cached).count();
        let failed = results.iter().filter(|r| !r.saved).count();

        Ok(AgentToolResult::success_json(serde_json::json!({
            "total": total,
            "saved": saved,
            "cached": cached,
            "failed": failed,
            "results": results,
            "message": format!(
                "{saved} saved, {cached} cached, {failed} failed out of {total} articles."
            ),
        })))
    }
}

impl BibSaveTool {
    /// Fetch + upsert a single article. Never errors — failures are captured
    /// in the returned [`SaveResult::error`] so one bad ID doesn't abort the batch.
    async fn save_one(
        &self,
        id: &str,
        source_override: Option<&str>,
    ) -> SaveResult {
        // 1. Check if already in local library.
        if let Some(kind) = detect_id_kind(id) {
            if let Ok(Some(existing)) = self.bib.find_by_identifier(kind, id).await {
                let doi = existing.doi().map(str::to_owned);
                let pmid = existing.pmid().map(str::to_owned);
                return SaveResult {
                    id: id.into(),
                    saved: true,
                    cached: true,
                    article_id: Some(existing.id),
                    source: None,
                    title: Some(existing.title),
                    doi,
                    pmid,
                    year: existing.year,
                    error: None,
                };
            }
        }

        // 2. Fetch from external source.
        let fetched = if let Some(source) = source_override {
            self.gateway
                .fetch_from(source, id)
                .await
                .ok()
                .flatten()
                .map(|article| (source.into(), article))
        } else {
            auto_fetch(&self.gateway, id).await
        };

        let (source_name, article) = match fetched {
            Some(x) => x,
            None => {
                return SaveResult {
                    id: id.into(),
                    saved: false,
                    cached: false,
                    article_id: None,
                    source: None,
                    title: None,
                    doi: None,
                    pmid: None,
                    year: None,
                    error: Some(format!("Article '{id}' not found in any source")),
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
            Ok(()) => SaveResult {
                id: id.into(),
                saved: true,
                cached: false,
                article_id: Some(article_id),
                source: Some(source_name),
                title: Some(title),
                doi,
                pmid,
                year,
                error: None,
            },
            Err(e) => SaveResult {
                id: id.into(),
                saved: false,
                cached: false,
                article_id: None,
                source: Some(source_name),
                title: Some(title),
                doi,
                pmid,
                year,
                error: Some(format!("Database error: {e}")),
            },
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
// bib_add_to_collection
// ===========================================================================

#[tool(
    name = "bib_add_to_collection",
    description = "Add an article (already saved via bib_save) to a collection with a \
                  semantic role describing why it's included. \
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
    #[desc = "Article ID (from bib_save or lit_search results)"]
    pub article_id: String,
    #[desc = "Role: \"requested\", \"referenced\", \"cited\", or \"background\". Default: referenced"]
    pub role: Option<String>,
    #[desc = "Note explaining the article's relevance to this collection"]
    pub note: Option<String>,
}

pub struct BibAddToCollectionTool {
    pub bib: Arc<BibBase>,
}

#[async_trait]
impl ToolFunction for BibAddToCollectionTool {
    type Input = BibAddToCollectionInput;

    async fn run(&self, input: Self::Input) -> Result<AgentToolResult, ToolError> {
        let role = parse_role(input.role.as_deref());

        self.bib
            .add_to_collection(
                &input.collection_id,
                &input.article_id,
                role,
                AddedBy::Agent,
                input.note.as_deref(),
            )
            .await
            .map_err(box_error)?;

        Ok(AgentToolResult::success_json(serde_json::json!({
            "added": true,
            "collection_id": input.collection_id,
            "article_id": input.article_id,
            "role": role.as_str(),
        })))
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
// bib_search_library — LIKE search across local library
// ===========================================================================

#[tool(
    name = "bib_search_library",
    description = "Search the local library for articles by keyword. Searches across \
                  titles, abstracts, and stored full-text content. Returns relevance-ranked \
                  results with snippets. \
                  \
                  Use this to find articles you've already saved (via bib_save). \
                  For searching external databases, use lit_search instead."
)]
pub struct BibSearchLibraryInput {
    #[desc = "Search query (matched against title, abstract, and full text)"]
    pub query: String,
    #[desc = "Maximum results (default 20)"]
    pub limit: Option<usize>,
}

pub struct BibSearchLibraryTool {
    pub bib: Arc<BibBase>,
}

#[async_trait]
impl ToolFunction for BibSearchLibraryTool {
    type Input = BibSearchLibraryInput;

    async fn run(&self, input: Self::Input) -> Result<AgentToolResult, ToolError> {
        let limit = input.limit.unwrap_or(20).clamp(1, 100);
        let hits = self
            .bib
            .search_articles(&input.query, limit)
            .await
            .map_err(box_error)?;

        let items: Vec<serde_json::Value> = hits
            .iter()
            .map(|h| {
                serde_json::json!({
                    "article_id": h.article_id,
                    "title": h.title,
                    "snippet": h.snippet,
                })
            })
            .collect();

        Ok(AgentToolResult::success_json(serde_json::json!({
            "query": input.query,
            "total": items.len(),
            "results": items,
        })))
    }
}

// ===========================================================================
// bib_get_article — full metadata + full text from local library
// ===========================================================================

#[tool(
    name = "bib_get_article",
    description = "Retrieve a full article from the local library, including metadata \
                  (title, authors, abstract, journal, identifiers) and stored full text \
                  if available. \
                  \
                  The article must have been saved via `bib_save` first. \
                  If full text has not been uploaded, the `has_fulltext` field will be false \
                  and `fulltext` will be null."
)]
pub struct BibGetArticleInput {
    #[desc = "Article ID (from bib_save or bib_search_library results)"]
    pub article_id: String,
    #[desc = "If true and full text is available, include the full text content in the response. Default: true"]
    pub include_fulltext: Option<bool>,
}

pub struct BibGetArticleTool {
    pub bib: Arc<BibBase>,
}

#[async_trait]
impl ToolFunction for BibGetArticleTool {
    type Input = BibGetArticleInput;

    async fn run(&self, input: Self::Input) -> Result<AgentToolResult, ToolError> {
        let article = self
            .bib
            .get_article(&input.article_id)
            .await
            .map_err(box_error)?
            .ok_or_else(|| ToolError::ExecutionFailed {
                source: format!(
                    "Article '{}' not found in local library. Use bib_save to add it first.",
                    input.article_id
                )
                .into(),
            })?;

        let include_ft = input.include_fulltext.unwrap_or(true);
        let fulltext = if include_ft {
            self.bib
                .get_fulltext(&input.article_id)
                .await
                .map_err(box_error)?
        } else {
            None
        };

        let has_fulltext = fulltext.is_some();
        let text_content = fulltext.as_ref().and_then(|ft| ft.text_content.clone());

        Ok(AgentToolResult::success_json(serde_json::json!({
            "article_id": article.id,
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
            "has_fulltext": has_fulltext,
            "fulltext_source": fulltext.as_ref().map(|ft| ft.source.as_str()),
            "fulltext": text_content,
        })))
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
/// Requires both [`BibBase`] (for storage) and [`LiteratureGateway`]
/// (for `bib_save` external fetching).
pub fn bib_library_registrations(
    bib: Arc<BibBase>,
    gateway: Arc<LiteratureGateway>,
) -> Vec<ToolRegistration> {
    use agentik_core::tools::ToolRegistration as R;
    vec![
        R::from(BibSaveTool {
            bib: bib.clone(),
            gateway: gateway.clone(),
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
/// # Example
///
/// ```no_run
/// # use std::sync::Arc;
/// # use bib_base::{BibBase, query::LiteratureGateway, bib_all_registrations};
/// # async fn example() {
/// let bib = Arc::new(BibBase::open_in_memory().await.unwrap());
/// let gateway = Arc::new(LiteratureGateway::new());
/// let tools = bib_all_registrations(bib, gateway);
/// # }
/// ```
pub fn bib_all_registrations(
    bib: Arc<BibBase>,
    gateway: Arc<LiteratureGateway>,
) -> Vec<ToolRegistration> {
    let mut tools = crate::tools::bib_query_registrations(gateway.clone());
    tools.extend(bib_library_registrations(bib, gateway));
    tools
}

// ===========================================================================
// Helpers
// ===========================================================================

/// Detect the [`IdKind`] from an ID string format.
fn detect_id_kind(id: &str) -> Option<IdKind> {
    if id.starts_with("10.") {
        Some(IdKind::Doi)
    } else if id.chars().all(|c| c.is_ascii_digit()) && !id.is_empty() {
        Some(IdKind::Pmid)
    } else {
        None
    }
}

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
