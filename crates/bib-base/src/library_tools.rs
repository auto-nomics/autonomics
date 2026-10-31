//! Library management agent tools — organize, annotate, and retrieve
//! articles from the local Turso-backed bibliography database.
//!
//! Saving literature into the library used to be the `bib_save` tool;
//! it has migrated to the DAG evidence channel: search/fetch via
//! `source_literature` / `source_literature_fetch`, then a `bib_save`
//! DAG node that consumes one `evidence` artifact and writes every
//! record into the same `BibBase`. Library *management* (collections,
//! annotations, full-text requests, exports, deletion) and
//! *retrieval* (`bib_search_library` / `bib_get_article`) remain tools.
//!
//! # Tool inventory
//!
//! | Tool                        | Purpose                                                |
//! |-----------------------------|--------------------------------------------------------|
//! | `bib_delete`                | Remove articles (id- or identifier-based; dry-run safe). |
//! | `bib_create_collection`     | Create a new collection.                                |
//! | `bib_manage_collection`     | Rename / move / trash / restore / erase a collection subtree. |
//! | `bib_empty_trash`           | Permanently erase all trashed collections (dry-run safe). |
//! | `bib_add_to_collection`     | Add one or more articles to a collection + roles.       |
//! | `bib_remove_from_collection`| Remove one or more articles from a collection.          |
//! | `bib_list_collection`       | List collections or articles within one.                |
//! | `bib_search_library`        | Fielded lexical search across local library (multi-query). |
//! | `bib_get_article`           | Batch metadata plus full-text file references.          |
//! | `bib_request_fulltext`      | Mark an article as needing full-text upload.            |
//! | `bib_add_note`              | Quote-anchored highlight (tool-verified text match) or comment. |
//! | `bib_read_figure`           | Read a figure image out of stored full text.             |
//! | `bib_export`                | Render citation formats and write them to the agent VFS. |

use std::sync::Arc;

use agentik_core::tools::{ToolError, ToolFunction, ToolRegistration};
use crate::shared::BibShared;
use agentik_proc::tool;
use agentik_sdk::types::{ToolResult as AgentToolResult, ToolResultBlock};
use async_trait::async_trait;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use bib_types::{
    AddedBy, ArticleRole, CollectionStatus, FetchStatus, IdKind, Identifier, TextFormat,
};

use crate::bib_base::BibBase;
use crate::collections::CollectionAddOutcome;

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
    #[desc = "Parent collection ID; omit for a library-root collection"]
    pub parent_id: Option<String>,
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
        col.parent_id = input.parent_id;

        self.bib.upsert_collection(&col).await.map_err(box_error)?;
        let col = self
            .bib
            .get_collection(&id)
            .await
            .map_err(box_error)?
            .ok_or_else(|| box_error(crate::error::Error::NotFound(id.clone())))?;

        Ok(AgentToolResult::success_json(serde_json::json!({
            "collection_id": id,
            "parent_id":col.parent_id,
            "key":col.key,
            "library_id":col.library_id,
            "name": col.name,
            "description": col.description,
            "status": "active",
        })))
    }
}

#[tool(
    name = "bib_manage_collection",
    description = "Rename, move, trash, restore or permanently erase a collection subtree. Moving changes only the parent. Trash and erase preserve articles and attachments. Erase requires the collection to be in trash first."
)]
pub struct BibManageCollectionInput {
    #[desc = "Collection public ID"]
    pub collection_id: String,
    #[desc = "One of rename, move, trash, restore, erase"]
    pub action: String,
    #[desc = "New name for rename"]
    pub name: Option<String>,
    #[desc = "Destination parent ID for move; omit to move to the library root"]
    pub parent_id: Option<String>,
}
pub struct BibManageCollectionTool {
    pub bib: Arc<BibBase>,
}
#[async_trait]
impl ToolFunction for BibManageCollectionTool {
    type Input = BibManageCollectionInput;
    async fn run(&self, input: Self::Input) -> Result<AgentToolResult, ToolError> {
        use crate::collections::CollectionChange;
        let change = match input.action.as_str() {
            "rename" => CollectionChange::Update {
                name: Some(input.name.ok_or_else(|| {
                    box_error(crate::error::Error::InvalidCollection(
                        "name is required".into(),
                    ))
                })?),
                parent_id: None,
            },
            "move" => CollectionChange::Update {
                name: None,
                parent_id: Some(input.parent_id),
            },
            "trash" => CollectionChange::Trash,
            "restore" => CollectionChange::Restore,
            "erase" => {
                let collections = self
                    .bib
                    .list_collections_including_deleted(None, true)
                    .await
                    .map_err(box_error)?;
                if !collections
                    .iter()
                    .any(|c| c.id == input.collection_id && c.deleted)
                {
                    return Err(box_error(crate::error::Error::CollectionConflict(
                        "erase requires a trashed collection".into(),
                    )));
                }
                CollectionChange::Erase
            }
            _ => {
                return Err(box_error(crate::error::Error::InvalidCollection(
                    "unknown action".into(),
                )));
            }
        };
        let affected = self
            .bib
            .change_collection(&input.collection_id, change)
            .await
            .map_err(box_error)?;
        Ok(AgentToolResult::success_json(
            serde_json::json!({"affected_ids":affected}),
        ))
    }
}

// ===========================================================================
// bib_empty_trash — batch-erase every trashed collection subtree
// ===========================================================================

#[tool(
    name = "bib_empty_trash",
    description = "Permanently erase every trashed collection at once — the agent-side \
                  equivalent of the library GUI's \"Empty trash\". Only trashed roots are \
                  erased: each erase cascades over its whole trashed subtree, so nested \
                  collections are covered automatically. Articles, attachments and notes \
                  are preserved; only the collections and their membership rows are removed. \
                  Safety: pass `confirm=true` to actually erase; the default `confirm=false` \
                  runs as a dry-run that reports what would be erased without modifying \
                  the database. A no-op when the trash is empty."
)]
pub struct BibEmptyTrashInput {
    #[desc = "Pass true to permanently erase the trash. Default false = dry-run preview."]
    pub confirm: Option<bool>,
}

pub struct BibEmptyTrashTool {
    pub bib: Arc<BibBase>,
}

#[async_trait]
impl ToolFunction for BibEmptyTrashTool {
    type Input = BibEmptyTrashInput;

    async fn run(&self, input: Self::Input) -> Result<AgentToolResult, ToolError> {
        use crate::collections::CollectionChange;
        let confirm = input.confirm.unwrap_or(false);
        let collections = self
            .bib
            .list_collections_including_deleted(None, true)
            .await
            .map_err(box_error)?;
        let trashed: Vec<&bib_types::Collection> =
            collections.iter().filter(|c| c.deleted).collect();
        let trashed_count = trashed.len();
        let trashed_ids: std::collections::HashSet<&String> =
            trashed.iter().map(|c| &c.id).collect();
        // Erase roots only: erase cascades over the trashed subtree, and
        // touching an already-erased descendant afterwards would fail NotFound.
        let roots: Vec<&bib_types::Collection> = trashed
            .into_iter()
            .filter(|c| {
                c.parent_id
                    .as_ref()
                    .is_none_or(|p| !trashed_ids.contains(p))
            })
            .collect();

        let mut erased_ids: Vec<String> = Vec::new();
        let mut failures: Vec<serde_json::Value> = Vec::new();
        if confirm {
            for root in &roots {
                match self
                    .bib
                    .change_collection(&root.id, CollectionChange::Erase)
                    .await
                {
                    Ok(ids) => erased_ids.extend(ids),
                    Err(e) => failures.push(serde_json::json!({
                        "collection_id": root.id,
                        "name": root.name,
                        "error": e.to_string(),
                    })),
                }
            }
        }

        let roots_json: Vec<serde_json::Value> = roots
            .iter()
            .map(|c| serde_json::json!({"collection_id": c.id, "name": c.name}))
            .collect();
        let message = if !confirm {
            if roots.is_empty() {
                "Trash is empty; nothing to erase.".to_owned()
            } else {
                format!(
                    "Dry run: the trash holds {} collection(s) across {} root subtree(s). \
                     Pass confirm=true to erase them permanently.",
                    trashed_count,
                    roots.len()
                )
            }
        } else if failures.is_empty() {
            format!(
                "Erased {} collection(s) from the trash. Articles, attachments and notes remain in the library.",
                erased_ids.len()
            )
        } else {
            format!(
                "Erased {} collection(s); {} root subtree(s) failed and remain in the trash.",
                erased_ids.len(),
                failures.len()
            )
        };

        Ok(AgentToolResult::success_json(serde_json::json!({
            "confirm": confirm,
            "trashed_collections": trashed_count,
            "roots": roots_json,
            "erased": erased_ids,
            "failed": failures,
            "message": message,
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
    /// Article ID (from a `bib_save` DAG node or `source_literature_fetch`).
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
    description = "Add one or more articles (already saved via a `bib_save` DAG node) to a collection, \
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
// bib_remove_from_collection — batch membership removal
// ===========================================================================

#[tool(
    name = "bib_remove_from_collection",
    description = "Remove one or more articles from a collection. Only the membership is \
                  deleted — the articles themselves stay in the library (use `bib_delete` \
                  to remove articles entirely). This is the counterpart of \
                  `bib_add_to_collection`; adding an article again later re-inserts it \
                  with fresh role/note values. Articles that are not members are reported \
                  as `was_member:false` instead of failing the batch. Removing from a \
                  trashed collection is rejected."
)]
pub struct BibRemoveFromCollectionInput {
    #[desc = "Collection ID (from bib_create_collection or bib_list_collection)"]
    pub collection_id: String,
    #[desc = "Article IDs to remove. Pass a list — even for a single article use a one-element list."]
    pub article_ids: Vec<String>,
}

pub struct BibRemoveFromCollectionTool {
    pub bib: Arc<BibBase>,
}

#[async_trait]
impl ToolFunction for BibRemoveFromCollectionTool {
    type Input = BibRemoveFromCollectionInput;

    async fn run(&self, input: Self::Input) -> Result<AgentToolResult, ToolError> {
        if input.article_ids.is_empty() {
            return Err(ToolError::ExecutionFailed {
                source: "at least one article id is required".into(),
            });
        }

        // One membership lookup up front: lets non-members be reported as
        // `was_member:false` instead of silently passing as removed.
        let members: std::collections::HashSet<String> = self
            .bib
            .list_collection_articles(&input.collection_id, None, None)
            .await
            .map_err(box_error)?
            .into_iter()
            .map(|a| a.article_id)
            .collect();

        let mut results = Vec::with_capacity(input.article_ids.len());
        for id in &input.article_ids {
            let was_member = members.contains(id);
            let outcome = if was_member {
                self.bib
                    .remove_from_collection(&input.collection_id, id)
                    .await
            } else {
                Ok(())
            };
            let error = outcome.err().map(|e| e.to_string());
            results.push(serde_json::json!({
                "article_id": id,
                "removed": was_member && error.is_none(),
                "was_member": was_member,
                "error": error,
            }));
        }

        let total = results.len();
        let removed = results
            .iter()
            .filter(|r| r["removed"] == serde_json::Value::Bool(true))
            .count();
        let not_member = results
            .iter()
            .filter(|r| r["was_member"] == serde_json::Value::Bool(false))
            .count();
        let failed = results.iter().filter(|r| r["error"].is_string()).count();

        Ok(AgentToolResult::success_json(serde_json::json!({
            "collection_id": input.collection_id,
            "total": total,
            "removed": removed,
            "not_member": not_member,
            "failed": failed,
            "results": results,
            "message": format!(
                "{removed} removed, {not_member} not a member, {failed} failed \
                 out of {total} articles in collection '{}'.",
                input.collection_id
            ),
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
    #[desc = "Include descendant collections when listing articles; default false"]
    pub include_descendants: Option<bool>,
    #[desc = "Include trashed collections in the collection listing; default false"]
    pub include_deleted: Option<bool>,
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
                let collections = self
                    .bib
                    .list_collections_including_deleted(
                        None,
                        input.include_deleted.unwrap_or(false),
                    )
                    .await
                    .map_err(box_error)?;

                let items: Vec<serde_json::Value> = collections
                    .iter()
                    .map(|c| {
                        serde_json::json!({
                            "collection_id": c.id,
                            "parent_id": c.parent_id,
                            "key": c.key,
                            "library_id":c.library_id,
                            "deleted":c.deleted,
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
                if input.include_descendants.unwrap_or(false) {
                    let page = self
                        .bib
                        .query_collection_articles(crate::collections::CollectionQuery {
                            collection_id: Some(cid.clone()),
                            include_descendants: true,
                            unfiled: false,
                            query: String::new(),
                            limit: i64::MAX as usize,
                            offset: 0,
                        })
                        .await
                        .map_err(box_error)?;
                    let associations = page
                        .associations
                        .into_iter()
                        .filter(|a| {
                            input.role.as_deref().is_none_or(|r| a.role.as_str() == r)
                                && input
                                    .fetch_status
                                    .as_deref()
                                    .is_none_or(|f| a.fetch_status.as_str() == f)
                        })
                        .collect::<Vec<_>>();
                    let articles = page
                        .articles
                        .into_iter()
                        .filter(|a| associations.iter().any(|ca| ca.article_id == a.id))
                        .collect::<Vec<_>>();
                    return Ok(AgentToolResult::success_json(
                        serde_json::json!({"collection_id":cid,"total":articles.len(),"articles":articles,"associations":associations}),
                    ));
                }
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
// bib_search_library — lexical search across local library (multi-query)
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
                  Supports field queries (title:, author:, year:, fulltext:), \
                  quoted phrases, parentheses, and AND/OR/NOT. \
                  \
                  Pass one or more queries — each is run concurrently against the local \
                  database and returns its own result group. Use multiple queries when \
                  you want to find articles matching several independent terms in one \
                  call instead of N separate tool calls. \
                  \
                  Use this to find articles you've already saved (via a `bib_save` DAG node). \
                  For searching external databases, use the DAG evidence channel \
                  (`source_literature`) instead. \
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
    /// Run a single query through the shared local search service. Failures land in
    /// `SearchQueryResult::error`.
    async fn search_one(&self, query: &str, limit: usize) -> SearchQueryResult {
        let hits = match self.bib.search_articles(query, limit, 0).await {
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
// bib_get_article — full metadata + full-text file refs from local library (batch)
// ===========================================================================

/// Result of fetching a single article within a batch. Never errors at the
/// tool level — failures (including "not found") are captured in `error` so
/// one bad ID doesn't abort the whole batch.
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
    /// The agent-readable full-text object: the canonical extracted text
    /// (`vfs:///literature/{id}/extracted/fulltext.{md|txt}`) for
    /// extracted rows with binary originals, or the stored file itself
    /// when it is already readable text. Read with the `vfs` tool.
    #[serde(skip_serializing_if = "Option::is_none")]
    fulltext_path: Option<String>,
    /// The uploaded original (`pdf`/`html`/`txt`) the extraction came
    /// from. Same object as `fulltext_path` when there is no separate
    /// extraction.
    #[serde(skip_serializing_if = "Option::is_none")]
    fulltext_original_path: Option<String>,
    /// `pdf` / `html` / `txt` — the stored object's file format.
    #[serde(skip_serializing_if = "Option::is_none")]
    fulltext_format: Option<String>,
    /// `sha256:{hex}` content hash when the object is content-addressed.
    #[serde(skip_serializing_if = "Option::is_none")]
    fulltext_hash: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    fulltext_size: Option<i64>,
    /// Extraction lifecycle: `pending` / `running` / `done` / `failed`.
    #[serde(skip_serializing_if = "Option::is_none")]
    fulltext_extract_status: Option<String>,
    /// `plain` / `markdown` — the extraction's text format when done.
    #[serde(skip_serializing_if = "Option::is_none")]
    fulltext_text_format: Option<String>,
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
            fulltext_path: None,
            fulltext_original_path: None,
            fulltext_format: None,
            fulltext_hash: None,
            fulltext_size: None,
            fulltext_extract_status: None,
            fulltext_text_format: None,
            annotations: Vec::new(),
            n_annotations: 0,
            error: Some(msg.into()),
        }
    }
}

#[tool(
    name = "bib_get_article",
    description = "Retrieve one or more articles from the local library, including metadata \
                  (title, authors, abstract, journal, identifiers) and a full-text **file \
                  reference** (VFS path, format, sha256, extraction status) when one is \
                  stored. \
                  \
                  Pass a list of article IDs — each is fetched concurrently from the \
                  local database. Articles must have been saved via a `bib_save` DAG \
                  node first; missing IDs are returned with `found: false` rather \
                  than failing the whole call. \
                  \
                  Full text is returned as the `fulltext_path` VFS URI, not inline \
                  text — read it with the `vfs` tool (`read`/`cat`/`grep` with \
                  offset+limit paging). Check `fulltext_extract_status` before \
                  reading: `pending`/`running` means extraction is still in flight, \
                  `failed` means no text could be extracted from the file. \
                  \
                  **Examples**: \
                  • article_ids=[\"a1\"] — fetch a single article \
                  • article_ids=[\"a1\", \"a2\", \"a3\"] — batch (fetched concurrently)"
)]
pub struct BibGetArticleInput {
    #[desc = "One or more article IDs (from a `bib_save` DAG node or `bib_search_library` results)"]
    pub article_ids: Vec<String>,
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

        // Fetch each article concurrently. The local DB is fast, but batching
        // still collapses N tool-call round-trips into one and lets the reads
        // overlap on the connection pool.
        let futures: Vec<_> = input
            .article_ids
            .iter()
            .map(|id| self.get_one(id.trim()))
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
    /// Fetch a single article + its full-text file reference + annotations.
    /// Never errors — failures are captured in `GetArticleResult::error` so
    /// one bad ID doesn't abort the batch.
    async fn get_one(&self, article_id: &str) -> GetArticleResult {
        let article = match self.bib.get_article(article_id).await {
            Ok(Some(a)) => a,
            Ok(None) => {
                return GetArticleResult::err(
                    article_id,
                    false,
                    format!(
                        "Article '{article_id}' not found in local library. \
                         Import it with a `bib_save` DAG node first."
                    ),
                );
            }
            Err(e) => return GetArticleResult::err(article_id, false, e.to_string()),
        };

        // The full-text row is the pointer: VFS path (or a legacy synthetic
        // pointer), format, hash, size, and the extraction lifecycle. The
        // inline `text_content` stays in the DB (FTS index food) — the agent
        // reads the file through the `vfs` tool instead.
        let fulltext = match self.bib.get_fulltext(article_id).await {
            Ok(ft) => ft,
            Err(e) => return GetArticleResult::err(article_id, false, e.to_string()),
        };
        let has_fulltext = fulltext.is_some();
        let fulltext_source = fulltext.as_ref().map(|ft| ft.source.as_str().to_owned());
        let fulltext_original_path = fulltext.as_ref().map(|ft| ft.file_path.clone());
        // Route `fulltext_path` at what the agent can actually read: for
        // extracted rows with binary originals (pdf/html) that is the
        // canonical extracted-text object; txt originals are readable as
        // stored, so they are their own entry point. The path is derived
        // by convention (`stored_files::extracted_object_path`) — legacy
        // `done` rows get theirs materialized by the startup backfill.
        let fulltext_path = fulltext.as_ref().map(|ft| {
            let extracted = ft.extract_status == Some(bib_types::ExtractStatus::Done)
                && ft.text_format.is_some()
                && matches!(
                    ft.file_format,
                    bib_types::FileFormat::Pdf | bib_types::FileFormat::Html
                );
            if extracted {
                let format = ft.text_format.unwrap_or(bib_types::TextFormat::Plain);
                match ft.attachment_key.as_deref() {
                    Some(key) => crate::stored_files::extracted_attachment_object_path(
                        &ft.article_id,
                        key,
                        format,
                    ),
                    None => crate::stored_files::extracted_object_path(&ft.article_id, format),
                }
            } else {
                ft.file_path.clone()
            }
        });
        let fulltext_format = fulltext
            .as_ref()
            .map(|ft| ft.file_format.as_str().to_owned());
        let fulltext_hash = fulltext
            .as_ref()
            .and_then(|ft| ft.file_hash.clone())
            .map(|hash| format!("sha256:{hash}"));
        let fulltext_size = fulltext.as_ref().and_then(|ft| ft.file_size);
        let fulltext_extract_status = fulltext
            .as_ref()
            .and_then(|ft| ft.extract_status)
            .map(|s| s.as_str().to_owned());
        let fulltext_text_format = fulltext
            .as_ref()
            .and_then(|ft| ft.text_format)
            .map(|f| f.as_str().to_owned());

        // Annotations (notes/highlights/comments) — without this the write
        // path (bib_add_note) is a data black hole: annotations are persisted
        // but never reachable through any read tool.
        let annotations = match self.bib.list_annotations(article_id, None).await {
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
            fulltext_path,
            fulltext_original_path,
            fulltext_format,
            fulltext_hash,
            fulltext_size,
            fulltext_extract_status,
            fulltext_text_format,
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
    #[desc = "Article ID (already in the local library via a `bib_save` DAG node)"]
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
    description = "Add a highlight or comment to an article in the library. A \
                  `highlight` anchors a passage of the paper itself: pass the \
                  exact sentence as `content` — copied verbatim from the \
                  paper's full text (case-sensitive; line breaks and extra \
                  spaces are ignored) — and the tool locates it in the PDF by \
                  text match, verifies it, and computes the highlight \
                  geometry. Never invent page numbers or coordinates. When \
                  the same words occur on several pages, pass `page` as a \
                  1-based hint. A `comment` is your own observation, key \
                  finding, or critique. Both are searchable alongside the \
                  article's text."
)]
pub struct BibAddNoteInput {
    #[desc = "Article ID (from a `bib_save` DAG node or `bib_search_library`)"]
    pub article_id: String,
    #[desc = "Annotation type: \"highlight\" (verbatim quote from the paper) or \"comment\" (your own remark). Default: comment"]
    pub kind: Option<String>,
    #[desc = "For a highlight: the passage copied exactly from the paper's full text. For a comment: your remark"]
    pub content: String,
    #[desc = "1-based PDF page hint, required when the same quote appears on several pages; the error lists the candidate pages"]
    pub page: Option<u32>,
    #[desc = "Highlight color (used by the PDF reader)"]
    pub color: Option<String>,
    #[desc = "Optional Zotero attachment item key; defaults to the preferred attachment"]
    pub attachment_key: Option<String>,
}

pub struct BibAddNoteTool {
    pub shared: Arc<BibShared>,
}

#[async_trait]
impl ToolFunction for BibAddNoteTool {
    type Input = BibAddNoteInput;

    async fn run(&self, input: Self::Input) -> Result<AgentToolResult, ToolError> {
        // Only two kinds exist: quoted excerpts (highlight) and user
        // remarks (comment). Legacy "note" callers — remarks, back when
        // note was its own kind — fold into comment; anything else is a
        // hard error rather than a silent default.
        let kind = match input
            .kind
            .as_deref()
            .map(str::trim)
            .map(str::to_ascii_lowercase)
            .as_deref()
        {
            None | Some("comment") | Some("note") => bib_types::AnnotationKind::Comment,
            Some("highlight") => bib_types::AnnotationKind::Highlight,
            Some(other) => {
                return Err(ToolError::ValidationFailed {
                    message: format!(
                        "unknown annotation kind `{other}`: expected \"highlight\" or \"comment\""
                    ),
                });
            }
        };
        match kind {
            bib_types::AnnotationKind::Comment => {
                let ann = self
                    .shared
                    .bib
                    .add_annotation(
                        &input.article_id,
                        kind,
                        input.content.trim(),
                        input.page,
                        input.color,
                        None,
                        input.attachment_key.as_deref(),
                    )
                    .await
                    .map_err(box_error)?;
                Ok(AgentToolResult::success_json(serde_json::json!({
                    "annotation_id": ann.id,
                    "article_id": input.article_id,
                    "kind": ann.kind.as_str(),
                    "content": ann.content,
                    "page": ann.page,
                    "color": ann.color,
                    "attachment_key": ann.attachment_key,
                })))
            }
            bib_types::AnnotationKind::Highlight => {
                let matched = self.resolve_highlight_anchor(&input).await?;
                // The reader stores one annotation per page for selections
                // that cross a page break; every row carries the full quote.
                let mut annotations = Vec::new();
                for anchor in &matched.page_anchors {
                    let ann = self
                        .shared
                        .bib
                        .add_annotation(
                            &input.article_id,
                            kind,
                            &matched.quote,
                            Some(anchor.page),
                            input.color.clone(),
                            Some(anchor.rects.clone()),
                            input.attachment_key.as_deref(),
                        )
                        .await
                        .map_err(box_error)?;
                    annotations.push(serde_json::json!({
                        "annotation_id": ann.id,
                        "page": anchor.page,
                        "page_quote": anchor.quote,
                        "n_rects": anchor.rects.len(),
                        "has_text_range": anchor
                            .rects
                            .first()
                            .and_then(|rect| rect.text_range.as_ref())
                            .is_some(),
                        "item_key": ann.item_key,
                        "attachment_key": ann.attachment_key,
                    }));
                }
                Ok(AgentToolResult::success_json(serde_json::json!({
                    "article_id": input.article_id,
                    "kind": "highlight",
                    "quote": matched.quote,
                    "pages": matched.pages,
                    "annotations": annotations,
                })))
            }
        }
    }
}

impl BibAddNoteTool {
    /// Locate the highlight `content` in the article's structured PDF text
    /// and derive validated anchors. Every failure mode is a
    /// [`ToolError::ValidationFailed`] carrying instructions, so the agent
    /// can correct the quote in its next call instead of persisting an
    /// anchor that would never render.
    async fn resolve_highlight_anchor(
        &self,
        input: &BibAddNoteInput,
    ) -> std::result::Result<crate::annotation_anchor::QuoteMatch, ToolError> {
        // Reading or (re)generating the SDT structure is the same path the
        // GUI's Reading Mode takes, including the generation mutex and the
        // shared extraction semaphore.
        let structure =
            crate::zotero_extract::structured_fulltext(
                &self.shared,
                &input.article_id,
                input.attachment_key.as_deref(),
                true,
            )
            .await
            .map_err(|error| ToolError::ValidationFailed {
                message: format!(
                    "cannot anchor a highlight on this article: {error}. A highlight quotes \
                     the paper's PDF full text — make sure the full text is stored, or use \
                     kind=\"comment\" for a remark that needs no page anchor"
                ),
            })?
            .ok_or_else(|| ToolError::ValidationFailed {
                message: "cannot anchor a highlight: the PDF structured text is unavailable"
                    .into(),
            })?;
        let index = crate::annotation_anchor::SdtTextIndex::build(&structure);
        let matched = index
            .find_quote(&input.content, input.page)
            .map_err(|error| ToolError::ValidationFailed {
                message: error.to_string(),
            })?;
        if matched.page_anchors.is_empty() {
            return Err(ToolError::ValidationFailed {
                message: "the quote matched text that has no highlightable PDF geometry; \
                          this document's text layer cannot be highlighted"
                    .into(),
            });
        }
        Ok(matched)
    }
}

// ===========================================================================
// bib_delete — remove articles from the local library
// ===========================================================================

/// A single typed article identifier used by [`BibDeleteTool`]'s `ids`
/// mode (typed external identifiers resolved via `find_by_identifier`).
#[derive(schemars::JsonSchema, serde::Deserialize, serde::Serialize)]
pub struct ArticleIdInput {
    #[schemars(
        description = "Identifier type: \"doi\", \"pmid\", \"pmc\", \"arxiv\", \"openalex\", \"s2\", or \"biorxiv\""
    )]
    pub id_type: String,
    #[schemars(
        description = "The identifier value (e.g. \"10.1038/...\", \"30124452\", \"2401.00001\")"
    )]
    pub id: String,
}

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
/// rows from the library. The `bib_save` DAG node is idempotent (matched
/// identifiers return as `cached` and cannot overwrite or replace) and
/// `bib_add_note` can only append annotations, so neither can be used to
/// evict a placeholder or mis-saved entry.
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
                  This is the only path that physically removes an article: the `bib_save` \
                  DAG node cannot overwrite or replace an existing row (matched identifiers \
                  are returned as `cached`), and `bib_add_note` can only append annotations. \
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
                  The rendered output is written directly to `output_path` in the \
                  agent-visible VFS. Existing files are replaced."
)]
pub struct BibExportInput {
    #[desc = "Include subcollection articles; default false"]
    pub include_descendants: Option<bool>,
    #[desc = "Destination path in the agent VFS, e.g. \"/outputs/references.bib\""]
    pub output_path: String,
    #[desc = "Collection ID to export. If omitted, exports entire library."]
    pub collection_id: Option<String>,
    #[desc = "Output format: \"bibtex\", \"ris\", \"markdown\", or \"csl_json\". Default: bibtex"]
    pub format: Option<String>,
    #[desc = "Maximum articles to export; omit to export the complete scope"]
    pub limit: Option<usize>,
}

pub struct BibExportTool {
    pub bib: Arc<BibBase>,
    pub storage: Arc<vfs::OpendalFileStorage>,
}

#[async_trait]
impl ToolFunction for BibExportTool {
    type Input = BibExportInput;

    async fn run(&self, input: Self::Input) -> Result<AgentToolResult, ToolError> {
        let output_path = input.output_path.trim().to_string();
        if output_path.is_empty() {
            return Err(ToolError::ExecutionFailed {
                source: "output_path must not be empty".into(),
            });
        }

        let format = parse_export_format(input.format.as_deref());
        let limit = input.limit.unwrap_or(i64::MAX as usize).max(1);
        let articles = self
            .bib
            .query_collection_articles(crate::collections::CollectionQuery {
                collection_id: input.collection_id,
                include_descendants: input.include_descendants.unwrap_or(false),
                unfiled: false,
                query: String::new(),
                limit,
                offset: 0,
            })
            .await
            .map_err(box_error)?
            .articles;

        let rendered = crate::export::render_all(&articles, format);
        let count = articles.len();
        let bytes = rendered.into_bytes();
        let size = bytes.len() as u64;

        self.storage
            .write_bytes(&output_path, bytes)
            .await
            .map_err(box_error)?;

        Ok(AgentToolResult::success_json(serde_json::json!({
            "format": format_extension(format),
            "count": count,
            "path": output_path,
            "size": size,
        })))
    }
}

// ===========================================================================
// bib_read_figure — deliver one extracted figure image to the agent
// ===========================================================================

/// Upper bound for figures entering model context. Extraction caps stored
/// figures at 15 MiB; this tighter limit keeps one oversized-but-legal
/// object from flooding a request.
const MAX_TOOL_FIGURE_BYTES: usize = 10 * 1024 * 1024;

#[tool(
    name = "bib_read_figure",
    description = "Read one figure image from an article's markdown full text and return it \
                  as an image block, for use with vision-capable models. \
                  \
                  Figure names appear inside the article's markdown full-text \
                  file (see its `fulltext_path` from `bib_get_article`, then read \
                  it with the `vfs` tool) as `images/<name>.jpg` references — \
                  pass either the bare name or the `images/`-prefixed \
                  form copied verbatim from the markdown. \
                  \
                  Only markdown extractions (MinerU layout-aware) carry figures; \
                  plain-text extractions have none. \
                  \
                  **Examples**: \
                  • figure=\"images/3a1f02c9.jpg\" — copied from the markdown \
                  • figure=\"3a1f02c9.jpg\" — bare name, same object"
)]
pub struct BibReadFigureInput {
    #[desc = "Article ID whose full text references the figure"]
    pub article_id: String,
    #[desc = "Figure file name — bare (\"abc.jpg\") or images/-prefixed, as it appears in the markdown"]
    pub figure: String,
}

/// Fetches one VFS-stored figure object as `[text summary, image block]`.
///
/// `storage` is `Option` so hosts without bibliography file storage can
/// still register the tool set; calling it there reports a clear error.
pub struct BibReadFigureTool {
    pub bib: Arc<BibBase>,
    pub storage: Option<Arc<vfs::OpendalFileStorage>>,
}

#[async_trait]
impl ToolFunction for BibReadFigureTool {
    type Input = BibReadFigureInput;

    async fn run(&self, input: Self::Input) -> Result<AgentToolResult, ToolError> {
        let article_id = input.article_id.trim();
        let fulltext = self
            .bib
            .get_fulltext(article_id)
            .await
            .map_err(box_error)?
            .ok_or_else(|| ToolError::ExecutionFailed {
                source: format!(
                    "no full text stored for '{article_id}' — save the article and upload its \
                     file first"
                )
                .into(),
            })?;
        if fulltext.text_format != Some(TextFormat::Markdown) {
            return Err(ToolError::ExecutionFailed {
                source: "this article's full text was not extracted as markdown, so it has no \
                         figures (only MinerU layout-aware extractions carry them)"
                    .into(),
            });
        }
        let name =
            crate::stored_files::sanitize_figure_name(input.figure.trim()).ok_or_else(|| {
                ToolError::ExecutionFailed {
                    source: "figure must be a plain file name ([A-Za-z0-9._-], no leading dot), \
                             e.g. \"abc123.jpg\" or \"images/abc123.jpg\""
                        .into(),
                }
            })?;
        let media_type = match name.rsplit('.').next().unwrap_or_default() {
            "jpg" | "jpeg" => "image/jpeg",
            "png" => "image/png",
            "gif" => "image/gif",
            "webp" => "image/webp",
            _ => {
                return Err(ToolError::ExecutionFailed {
                    source: format!(
                        "unsupported figure type for '{name}' (expected jpg, png, gif, or webp)"
                    )
                    .into(),
                });
            }
        };
        let storage = self
            .storage
            .as_deref()
            .ok_or_else(|| ToolError::ExecutionFailed {
                source: "bibliography VFS storage is not configured in this host".into(),
            })?;
        let object = match fulltext.attachment_key.as_deref() {
            Some(key) => {
                crate::stored_files::attachment_figure_object_path(&fulltext.article_id, key, &name)
            }
            None => crate::stored_files::figure_object_path(&fulltext.article_id, &name),
        };
        let path = crate::stored_files::vfs_virtual_path(&object).ok_or_else(|| {
            ToolError::ExecutionFailed {
                source: "figure object path is not VFS-addressable".into(),
            }
        })?;
        let length = storage.content_length(&path).await.map_err(|error| {
            if error.kind() == opendal::ErrorKind::NotFound {
                ToolError::ExecutionFailed {
                    source: format!(
                        "figure '{name}' not found — copy the exact name from the markdown's \
                         images/ references (read the fulltext_path file via the vfs tool)"
                    )
                    .into(),
                }
            } else {
                ToolError::ExecutionFailed {
                    source: error.to_string().into(),
                }
            }
        })?;
        if length as usize > MAX_TOOL_FIGURE_BYTES {
            return Err(ToolError::ExecutionFailed {
                source: format!(
                    "figure '{name}' is {length} bytes, above the {MAX_TOOL_FIGURE_BYTES} byte \
                     tool limit"
                )
                .into(),
            });
        }
        let data = crate::extraction::read_full(storage, &path)
            .await
            .map_err(|error| ToolError::ExecutionFailed {
                source: error.to_string().into(),
            })?;

        let summary = serde_json::json!({
            "article_id": fulltext.article_id,
            "figure": name,
            "bytes": data.len(),
            "media_type": media_type,
        });
        Ok(AgentToolResult::with_blocks(vec![
            ToolResultBlock::text(summary.to_string()),
            ToolResultBlock::image_base64(media_type, STANDARD.encode(&data)),
        ]))
    }
}

// ===========================================================================
// Registration
// ===========================================================================

/// Build [`ToolRegistration`]s for all library management tools.
///
/// Requires a [`BibBase`] (for storage) and the VFS file storage (for
/// `bib_read_figure` — pass a no-op handle when the host runs without
/// bibliography file storage).
///
/// `bib_save` is no longer a tool — fetching literature + writing it into
/// the library now flows through the DAG evidence channel: `bib_save` is a
/// `bib_save` DAG node that consumes an `evidence` artifact and writes
/// every record into the same `BibBase`. Library *retrieval* (`bib_search`
/// / `bib_get_article` / etc.) and *management* (`bib_create_collection`
/// / `bib_export` / etc.) remain tools; the agent invokes them as before.
pub fn bib_library_registrations(
    bib: Arc<BibBase>,
    shared: Arc<BibShared>,
    file_storage: Arc<vfs::OpendalFileStorage>,
) -> Vec<ToolRegistration> {
    use agentik_core::tools::ToolRegistration as R;
    vec![
        R::from(BibCreateCollectionTool { bib: bib.clone() }),
        R::from(BibManageCollectionTool { bib: bib.clone() }),
        R::from(BibEmptyTrashTool { bib: bib.clone() }),
        R::from(BibAddToCollectionTool { bib: bib.clone() }),
        R::from(BibRemoveFromCollectionTool { bib: bib.clone() }),
        R::from(BibListCollectionTool { bib: bib.clone() }),
        R::from(BibSearchLibraryTool { bib: bib.clone() }),
        R::from(BibGetArticleTool { bib: bib.clone() }),
        R::from(BibRequestFulltextTool { bib: bib.clone() }),
        R::from(BibAddNoteTool { shared: shared.clone() }),
        R::from(BibDeleteTool { bib: bib.clone() }),
        R::from(BibReadFigureTool {
            bib: bib.clone(),
            storage: Some(file_storage.clone()),
        }),
        R::from(BibExportTool {
            bib,
            storage: file_storage.clone(),
        }),
    ]
}

/// Build [`ToolRegistration`]s for **all** bibliography tools — both query
/// and management. This is the one-stop registration function.
///
/// `bib_save` is no longer a tool — fetching literature + writing it into
/// the library now flows through the DAG evidence channel; see
/// [`bib_library_registrations`] for details.
///
/// # Example
///
/// ```no_run
/// # use std::sync::Arc;
/// # use bib_base::{BibShared, bib_all_registrations};
/// # async fn example() {
/// let shared = BibShared::open_in_memory().await.unwrap()
///     .with_file_storage(Arc::new(vfs::OpendalFileStorage::new_temp()));
/// let tools = bib_all_registrations(Arc::new(shared));
/// # }
/// ```
pub fn bib_all_registrations(shared: Arc<BibShared>) -> Vec<ToolRegistration> {
    // Highlights resolve their anchors through the shared bundle (VFS storage
    // + extraction permits), so a bare BibBase is not enough here.
    let file_storage = shared.file_storage.clone().unwrap_or_else(|| {
        // A host that never attached storage still gets the read-only tools;
        // only export/figure writes would target an ephemeral VFS.
        Arc::new(vfs::OpendalFileStorage::new_temp())
    });
    // Literature *retrieval* now flows through the DAG evidence channel
    // (source_literature / source_literature_fetch / citation-graph nodes);
    // this registration covers only the local-library management tools.
    bib_library_registrations(shared.bib.clone(), shared, file_storage)
}

// ===========================================================================
// Helpers
// ===========================================================================

/// Parse a user-supplied identifier-kind string (formerly in the retired
/// `tools` module; kept for the library tools' by-id lookups).
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

    // ── bib_export ────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_export_writes_bibtex_to_vfs() {
        let (bib, _) = seed_one_article().await;
        let storage = Arc::new(vfs::OpendalFileStorage::new_temp());
        let tool = BibExportTool {
            bib,
            storage: storage.clone(),
        };

        let input = BibExportInput {
            include_descendants: None,
            output_path: "/references.bib".into(),
            collection_id: None,
            format: Some("bibtex".into()),
            limit: Some(100),
        };

        let result = tool.run(input).await.unwrap();
        let json = match result.content {
            ToolResultContent::Json(v) => v,
            _ => panic!("expected JSON"),
        };

        assert_eq!(json["count"].as_u64(), Some(1));
        assert_eq!(json["format"].as_str(), Some("bib"));
        assert_eq!(json["path"].as_str(), Some("/references.bib"));
        assert!(json.get("export").is_none());

        let size = json["size"].as_u64().unwrap();
        let bytes = storage
            .read_range("/references.bib", 0..size)
            .await
            .unwrap();
        let text = String::from_utf8(bytes.to_vec()).unwrap();
        assert!(text.starts_with("@article{"));
        assert!(text.contains("Test paper"));
        assert!(text.contains("10.1038/ng.2024.999"));
    }

    #[tokio::test]
    async fn test_export_rejects_blank_output_path() {
        let bib = Arc::new(BibBase::open_in_memory().await.unwrap());
        let tool = BibExportTool {
            bib,
            storage: Arc::new(vfs::OpendalFileStorage::new_temp()),
        };

        let input = BibExportInput {
            include_descendants: None,
            output_path: "   ".into(),
            collection_id: None,
            format: None,
            limit: None,
        };

        assert!(tool.run(input).await.is_err());
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

    // ── bib_add_note ──────────────────────────────────────────────────────

    /// One horizontal textMap run: every non-whitespace character gets a
    /// 10-unit box advancing from x=50, line box y..y+12, on `page` (0-based).
    fn flat_text_map(text: &str, page: u32, y: f64) -> String {
        let mut run = vec![
            serde_json::json!(0u64),
            serde_json::json!(page),
            serde_json::json!(50.0),
            serde_json::json!(y),
            serde_json::json!(50.0),
            serde_json::json!(y + 12.0),
        ];
        run.extend(
            text.chars()
                .filter(|ch| !matches!(ch, ' ' | '\n' | '\t'))
                .map(|_| serde_json::json!(10.0)),
        );
        serde_json::to_string(&vec![serde_json::Value::Array(run)]).unwrap()
    }

    /// A minimal valid SDT document (`StructuredCache::matches` requires the
    /// schema version, processor type/version, content and catalog pages).
    fn sdt_structure(content: serde_json::Value, pages: usize) -> serde_json::Value {
        serde_json::json!({
            "schemaVersion": "1.2.0",
            "metadata": {
                "source": { "hash": "hash-sdt" },
                "processor": { "type": "pdf", "version": 15 },
            },
            "content": content,
            "catalog": {
                "pages": (0..pages).map(|_| serde_json::json!({
                    "viewRect": [0.0, 0.0, 612.0, 792.0],
                })).collect::<Vec<_>>(),
            },
        })
    }

    /// Seed an article with a stored PDF fulltext row plus a cached SDT
    /// structure, so highlight anchoring resolves without running the real
    /// Zotero extractor.
    async fn seed_article_with_sdt(
        shared: &Arc<BibShared>,
        article_id: &str,
        structure: serde_json::Value,
    ) {
        let fulltext = bib_types::FullText {
            article_id: article_id.to_owned(),
            attachment_key: None,
            file_path: format!("vfs:///bib/{article_id}/paper.pdf"),
            file_format: bib_types::FileFormat::Pdf,
            text_content: None,
            source: bib_types::FullTextSource::UserUpload,
            file_hash: Some(format!("hash-{article_id}")),
            file_size: None,
            uploaded_at: None,
            extract_status: None,
            text_format: None,
            extracted_by: None,
            extract_error: None,
        };
        shared.bib.upsert_fulltext(&fulltext).await.unwrap();
        // upsert_fulltext links (or creates) a Zotero attachment, and the
        // structured cache is keyed by that attachment — mirror the reader's
        // Reading Mode path so `structured_fulltext` finds the cache.
        let key = shared
            .bib
            .zotero_attachment(article_id)
            .await
            .unwrap()
            .map(|attachment| attachment.key);
        let storage = shared.file_storage.clone().expect("test storage attached");
        crate::zotero_extract::store_structure(
            &storage,
            article_id,
            key.as_deref(),
            &fulltext,
            Some(&structure),
        )
        .await
        .unwrap();
    }


    async fn shared_with_storage() -> Arc<BibShared> {
        Arc::new(
            BibShared::open_in_memory()
                .await
                .unwrap()
                .with_file_storage(Arc::new(vfs::OpendalFileStorage::new_temp())),
        )
    }

    #[tokio::test]
    async fn test_add_note_highlight_anchors_quote_by_text_match() {
        let shared = shared_with_storage().await;
        let art = bib_types::Article::new("doi:10.1038/ng.2024.hl", "Highlight paper");
        shared.bib.upsert_article(&art).await.unwrap();
        let text = "The mitochondrion is the powerhouse of the cell";
        seed_article_with_sdt(
            &shared,
            &art.id,
            sdt_structure(
                serde_json::json!([{
                    "type": "paragraph",
                    "content": [{
                        "text": text,
                        "anchor": { "textMap": flat_text_map(text, 0, 700.0) },
                    }],
                }]),
                1,
            ),
        )
        .await;

        let tool = BibAddNoteTool { shared: shared.clone() };
        let result = tool
            .run(BibAddNoteInput {
                article_id: art.id.clone(),
                kind: Some("highlight".into()),
                content: "mitochondrion is the".into(),
                page: None,
                color: Some("#ffd400".into()),
                attachment_key: None,
            })
            .await
            .unwrap();
        match result.content {
            ToolResultContent::Json(value) => {
                assert_eq!(value["kind"], "highlight");
                assert_eq!(value["quote"], "mitochondrion is the");
                assert_eq!(value["pages"][0], 1);
                assert_eq!(value["annotations"][0]["page"], 1);
                assert_eq!(value["annotations"][0]["n_rects"], 1);
                assert_eq!(value["annotations"][0]["has_text_range"], true);
            }
            other => panic!("unexpected tool result: {other:?}"),
        }

        let annotations = shared.bib.list_annotations(&art.id, None).await.unwrap();
        assert_eq!(annotations.len(), 1);
        let ann = &annotations[0];
        assert_eq!(ann.kind.as_str(), "highlight");
        assert_eq!(ann.page, Some(1));
        assert_eq!(ann.content, "mitochondrion is the");
        assert_eq!(ann.color.as_deref(), Some("#ffd400"));
        let rects = ann.rects.as_ref().expect("highlight carries rects");
        assert_eq!(rects.len(), 1);
        assert_eq!(rects[0].space.as_deref(), Some("pdf"));
        // 'm' is the fourth UTF-16 unit; the quote spans 20 units exclusive.
        let range = rects[0].text_range.as_ref().unwrap();
        assert_eq!(range.start, vec![0, 0, 4]);
        assert_eq!(range.end, vec![0, 0, 24]);
        assert_eq!(range.source_hash, "hash-sdt");
    }

    #[tokio::test]
    async fn test_add_note_highlight_rejects_quote_not_in_paper() {
        let shared = shared_with_storage().await;
        let art = bib_types::Article::new("doi:10.1038/ng.2024.miss", "Missing quote paper");
        shared.bib.upsert_article(&art).await.unwrap();
        let text = "Actual sentence in the paper.";
        seed_article_with_sdt(
            &shared,
            &art.id,
            sdt_structure(
                serde_json::json!([{
                    "type": "paragraph",
                    "content": [{
                        "text": text,
                        "anchor": { "textMap": flat_text_map(text, 0, 700.0) },
                    }],
                }]),
                1,
            ),
        )
        .await;

        let tool = BibAddNoteTool { shared: shared.clone() };
        let error = tool
            .run(BibAddNoteInput {
                article_id: art.id.clone(),
                kind: Some("highlight".into()),
                content: "words that appear nowhere in the document".into(),
                page: None,
                color: None,
                attachment_key: None,
            })
            .await
            .unwrap_err();
        match error {
            ToolError::ValidationFailed { message } => {
                assert!(message.contains("quote not found"), "{message}");
                assert!(message.contains("verbatim"), "{message}");
            }
            other => panic!("expected ValidationFailed, got {other:?}"),
        }
        // Nothing was persisted.
        assert!(
            shared
                .bib
                .list_annotations(&art.id, None)
                .await
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn test_add_note_highlight_reports_ambiguous_pages() {
        let shared = shared_with_storage().await;
        let art = bib_types::Article::new("doi:10.1038/ng.2024.amb", "Repeated quote paper");
        shared.bib.upsert_article(&art).await.unwrap();
        let repeated = "Significant result reported in the study.";
        seed_article_with_sdt(
            &shared,
            &art.id,
            sdt_structure(
                serde_json::json!([
                    {
                        "type": "paragraph",
                        "content": [{
                            "text": repeated,
                            "anchor": { "textMap": flat_text_map(repeated, 0, 700.0) },
                        }],
                    },
                    {
                        "type": "paragraph",
                        "content": [{
                            "text": repeated,
                            "anchor": { "textMap": flat_text_map(repeated, 1, 700.0) },
                        }],
                    },
                ]),
                2,
            ),
        )
        .await;

        let tool = BibAddNoteTool { shared: shared.clone() };
        let error = tool
            .run(BibAddNoteInput {
                article_id: art.id.clone(),
                kind: Some("highlight".into()),
                content: repeated.into(),
                page: None,
                color: None,
                attachment_key: None,
            })
            .await
            .unwrap_err();
        match error {
            ToolError::ValidationFailed { message } => {
                assert!(message.contains("2 locations"), "{message}");
                assert!(message.contains("pages: 1, 2"), "{message}");
            }
            other => panic!("expected ValidationFailed, got {other:?}"),
        }
        // The same call with a page hint anchors only that page.
        let result = tool
            .run(BibAddNoteInput {
                article_id: art.id.clone(),
                kind: Some("highlight".into()),
                content: repeated.into(),
                page: Some(2),
                color: None,
                attachment_key: None,
            })
            .await
            .unwrap();
        match result.content {
            ToolResultContent::Json(value) => {
                assert_eq!(value["pages"][0], 2);
            }
            other => panic!("unexpected tool result: {other:?}"),
        }
        let annotations = shared.bib.list_annotations(&art.id, None).await.unwrap();
        assert_eq!(annotations.len(), 1);
        assert_eq!(annotations[0].page, Some(2));
    }

    #[tokio::test]
    async fn test_add_note_comment_needs_no_pdf() {
        let shared = shared_with_storage().await;
        let art = bib_types::Article::new("doi:10.1038/ng.2024.cmt", "Comment only paper");
        shared.bib.upsert_article(&art).await.unwrap();

        let tool = BibAddNoteTool { shared: shared.clone() };
        let result = tool
            .run(BibAddNoteInput {
                article_id: art.id.clone(),
                kind: Some("comment".into()),
                content: "Methods are thin; the cohort is underpowered.".into(),
                page: None,
                color: None,
                attachment_key: None,
            })
            .await
            .unwrap();
        match result.content {
            ToolResultContent::Json(value) => {
                assert_eq!(value["kind"], "comment");
                assert!(value["annotation_id"].as_str().is_some());
            }
            other => panic!("unexpected tool result: {other:?}"),
        }
        let annotations = shared.bib.list_annotations(&art.id, None).await.unwrap();
        assert_eq!(annotations.len(), 1);
        assert_eq!(annotations[0].kind.as_str(), "comment");
        assert_eq!(annotations[0].rects, None);
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
        bib.add_annotation(
            &art.id,
            bib_types::AnnotationKind::Comment,
            "stale note",
            None,
            None,
            None,
            None,
        )
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
        assert_eq!(bib.list_annotations(&art.id, None).await.unwrap().len(), 1);

        let tool = BibDeleteTool { bib: bib.clone() };
        let input = BibDeleteInput {
            article_ids: Some(vec![art.id.clone()]),
            ids: None,
            confirm: Some(true),
        };
        tool.run(input).await.unwrap();

        // FK CASCADE should have wiped everything.
        assert_eq!(bib.article_count().await.unwrap(), 0);
        assert_eq!(bib.list_annotations(&art.id, None).await.unwrap().len(), 0);
        assert_eq!(
            bib.list_collection_articles(&col.id, None, None)
                .await
                .unwrap()
                .len(),
            0
        );
    }

    // ── bib_remove_from_collection ────────────────────────────────────────

    #[tokio::test]
    async fn test_remove_from_collection_batch() {
        let bib = Arc::new(BibBase::open_in_memory().await.unwrap());
        let mut art = bib_types::Article::new("doi:10.1/remove-a", "Member paper");
        art.identifiers
            .push(bib_types::Identifier::doi("10.1/remove-a"));
        bib.upsert_article(&art).await.unwrap();
        let stray = bib_types::Article::new("doi:10.1/remove-b", "Non-member paper");
        bib.upsert_article(&stray).await.unwrap();
        let col = bib_types::Collection::new("col-remove", "remove-target");
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

        let tool = BibRemoveFromCollectionTool { bib: bib.clone() };
        let input = BibRemoveFromCollectionInput {
            collection_id: col.id.clone(),
            article_ids: vec![art.id.clone(), stray.id.clone()],
        };
        let result = tool.run(input).await.unwrap();
        let json = match result.content {
            ToolResultContent::Json(v) => v,
            _ => panic!("expected JSON"),
        };

        assert_eq!(json["total"].as_u64(), Some(2));
        assert_eq!(json["removed"].as_u64(), Some(1));
        assert_eq!(json["not_member"].as_u64(), Some(1));
        assert_eq!(json["failed"].as_u64(), Some(0));
        assert_eq!(json["results"][0]["removed"], serde_json::Value::Bool(true));
        assert_eq!(
            json["results"][1]["was_member"],
            serde_json::Value::Bool(false)
        );

        // Membership is gone; the articles themselves remain in the library.
        assert_eq!(
            bib.list_collection_articles(&col.id, None, None)
                .await
                .unwrap()
                .len(),
            0
        );
        assert_eq!(bib.article_count().await.unwrap(), 2);
    }

    #[tokio::test]
    async fn test_remove_rejects_empty_input() {
        let bib = Arc::new(BibBase::open_in_memory().await.unwrap());
        let tool = BibRemoveFromCollectionTool { bib };
        let err = tool
            .run(BibRemoveFromCollectionInput {
                collection_id: "col".into(),
                article_ids: vec![],
            })
            .await;
        assert!(err.is_err());
    }

    // ── bib_empty_trash ───────────────────────────────────────────────────

    #[tokio::test]
    async fn test_empty_trash_dry_run_then_confirm() {
        let bib = Arc::new(BibBase::open_in_memory().await.unwrap());
        let art = bib_types::Article::new("doi:10.1/trash-keep", "Survivor paper");
        bib.upsert_article(&art).await.unwrap();
        let parent = bib_types::Collection::new("col-trash-root", "trashed root");
        bib.upsert_collection(&parent).await.unwrap();
        let mut child = bib_types::Collection::new("col-trash-kid", "trashed child");
        child.parent_id = Some(parent.id.clone());
        bib.upsert_collection(&child).await.unwrap();
        bib.add_to_collection(
            &child.id,
            &art.id,
            ArticleRole::Referenced,
            AddedBy::Agent,
            None,
        )
        .await
        .unwrap();
        bib.change_collection(&parent.id, crate::collections::CollectionChange::Trash)
            .await
            .unwrap();

        let tool = BibEmptyTrashTool { bib: bib.clone() };

        // Dry run previews the root subtree without touching the database.
        let dry = tool
            .run(BibEmptyTrashInput { confirm: None })
            .await
            .unwrap();
        let json = match dry.content {
            ToolResultContent::Json(v) => v,
            _ => panic!("expected JSON"),
        };
        assert_eq!(json["confirm"], serde_json::Value::Bool(false));
        assert_eq!(json["trashed_collections"].as_u64(), Some(2));
        assert_eq!(json["roots"].as_array().unwrap().len(), 1);
        assert_eq!(
            json["roots"][0]["collection_id"].as_str(),
            Some(parent.id.as_str())
        );
        assert!(json["erased"].as_array().unwrap().is_empty());
        assert_eq!(
            bib.list_collections_including_deleted(None, true)
                .await
                .unwrap()
                .len(),
            2
        );

        // Confirm erases the whole subtree but preserves the articles.
        let sure = tool
            .run(BibEmptyTrashInput {
                confirm: Some(true),
            })
            .await
            .unwrap();
        let json = match sure.content {
            ToolResultContent::Json(v) => v,
            _ => panic!("expected JSON"),
        };
        assert_eq!(json["confirm"], serde_json::Value::Bool(true));
        let erased: Vec<&str> = json["erased"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert!(erased.contains(&parent.id.as_str()));
        assert!(erased.contains(&child.id.as_str()));
        assert_eq!(json["failed"].as_array().unwrap().len(), 0);
        assert_eq!(
            bib.list_collections_including_deleted(None, true)
                .await
                .unwrap()
                .len(),
            0
        );
        assert_eq!(bib.article_count().await.unwrap(), 1);
    }

    #[tokio::test]
    async fn test_empty_trash_is_noop_when_empty() {
        let bib = Arc::new(BibBase::open_in_memory().await.unwrap());
        let tool = BibEmptyTrashTool { bib: bib.clone() };
        let result = tool
            .run(BibEmptyTrashInput {
                confirm: Some(true),
            })
            .await
            .unwrap();
        let json = match result.content {
            ToolResultContent::Json(v) => v,
            _ => panic!("expected JSON"),
        };
        assert_eq!(json["trashed_collections"].as_u64(), Some(0));
        assert!(json["erased"].as_array().unwrap().is_empty());
    }

    // ── bib_read_figure ──────────────────────────────────────────────────

    async fn figure_tool_with_seeded_storage() -> (
        BibReadFigureTool,
        Arc<BibBase>,
        Arc<vfs::OpendalFileStorage>,
    ) {
        let bib = Arc::new(BibBase::open_in_memory().await.unwrap());
        let storage = Arc::new(vfs::OpendalFileStorage::new_temp());
        let article = bib_types::Article::new("doi:10.1/figure-tool", "Figure tool test");
        bib.upsert_article(&article).await.unwrap();
        // A full-text row must exist before its extraction outcome can be
        // recorded (record_extraction_success only UPDATEs).
        bib.upsert_fulltext(&bib_types::FullText {
            article_id: article.id.clone(),
            attachment_key: None,
            file_path: "vfs:///literature/seeded/paper.pdf".to_owned(),
            file_format: bib_types::FileFormat::Pdf,
            text_content: None,
            source: bib_types::FullTextSource::UserUpload,
            file_hash: None,
            file_size: None,
            uploaded_at: None,
            extract_status: None,
            text_format: None,
            extracted_by: None,
            extract_error: None,
        })
        .await
        .unwrap();
        bib.record_extraction_success(
            &article.id,
            "![](images/aaa.jpg)",
            TextFormat::Markdown,
            "test-markdown",
        )
        .await
        .unwrap();
        let attachment_key = bib
            .get_fulltext(&article.id)
            .await
            .unwrap()
            .unwrap()
            .attachment_key
            .unwrap();
        let path = crate::stored_files::vfs_virtual_path(
            &crate::stored_files::attachment_figure_object_path(
                &article.id,
                &attachment_key,
                "aaa.jpg",
            ),
        )
        .unwrap();
        storage.write_bytes(&path, vec![1, 2, 3, 4]).await.unwrap();
        (
            BibReadFigureTool {
                bib: bib.clone(),
                storage: Some(storage.clone()),
            },
            bib,
            storage,
        )
    }

    #[tokio::test]
    async fn read_figure_returns_text_and_image_blocks() {
        let (tool, _, _) = figure_tool_with_seeded_storage().await;

        let result = tool
            .run(BibReadFigureInput {
                article_id: "doi:10.1/figure-tool".into(),
                figure: "images/aaa.jpg".into(),
            })
            .await
            .unwrap();

        let blocks = match result.content {
            ToolResultContent::Blocks(blocks) => blocks,
            other => panic!("expected blocks, got {other:?}"),
        };
        assert_eq!(blocks.len(), 2);
        assert!(matches!(&blocks[0], ToolResultBlock::Text { text } if text.contains("aaa.jpg")));
        match &blocks[1] {
            ToolResultBlock::Image {
                source: agentik_sdk::types::ToolImageSource::Base64 { media_type, data },
            } => {
                assert_eq!(media_type, "image/jpeg");
                assert_eq!(
                    data.as_str(),
                    STANDARD.encode([1, 2, 3, 4]),
                    "image payload should be the stored bytes, base64-encoded"
                );
            }
            other => panic!("expected an image block, got {other:?}"),
        }

        // The bare name without the images/ prefix resolves to the same object.
        let result = tool
            .run(BibReadFigureInput {
                article_id: "doi:10.1/figure-tool".into(),
                figure: "aaa.jpg".into(),
            })
            .await
            .unwrap();
        assert!(matches!(result.content, ToolResultContent::Blocks(_)));
    }

    #[tokio::test]
    async fn read_figure_errors_are_actionable() {
        let (tool, bib, _) = figure_tool_with_seeded_storage().await;

        // Unknown article.
        let error = tool
            .run(BibReadFigureInput {
                article_id: "doi:10.1/missing".into(),
                figure: "aaa.jpg".into(),
            })
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("no full text"), "error was: {error}");

        // Plain-text extraction: no figures at all.
        bib.record_extraction_success(
            "doi:10.1/figure-tool",
            "plain words",
            TextFormat::Plain,
            "simple",
        )
        .await
        .unwrap();
        let error = tool
            .run(BibReadFigureInput {
                article_id: "doi:10.1/figure-tool".into(),
                figure: "aaa.jpg".into(),
            })
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("markdown"), "error was: {error}");

        // Restore markdown; now a missing figure and a bad name.
        bib.record_extraction_success(
            "doi:10.1/figure-tool",
            "![](images/aaa.jpg)",
            TextFormat::Markdown,
            "test-markdown",
        )
        .await
        .unwrap();
        let error = tool
            .run(BibReadFigureInput {
                article_id: "doi:10.1/figure-tool".into(),
                figure: "nope.jpg".into(),
            })
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("not found"), "error was: {error}");
        // Rejected outright: not a plain file name.
        let error = tool
            .run(BibReadFigureInput {
                article_id: "doi:10.1/figure-tool".into(),
                figure: "..".into(),
            })
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("plain file name"), "error was: {error}");
        // Traversal is neutralized to the basename, then fails the type check.
        let error = tool
            .run(BibReadFigureInput {
                article_id: "doi:10.1/figure-tool".into(),
                figure: "../etc/passwd".into(),
            })
            .await
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("unsupported figure type"),
            "error was: {error}"
        );
    }
}
