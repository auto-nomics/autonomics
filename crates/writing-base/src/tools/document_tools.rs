//! Document management tools — create, list, open, delete, outline, metadata.

use std::sync::Arc;

use agentik_core::tools::{ToolError, ToolFunction, ToolRegistration};
use agentik_proc::tool;
use agentik_sdk::types::ToolResult as AgentToolResult;
use async_trait::async_trait;
use writing_types::{DocumentAuthor, DocumentClass, SectionLevel};

use super::WritingToolState;
use crate::ast;
use crate::store::WritingStore;

/// Build all document-management tool registrations.
pub fn registrations(state: WritingToolState) -> Vec<ToolRegistration> {
    vec![
        ToolRegistration::from(DocCreateTool {
            store: state.store.clone(),
        }),
        ToolRegistration::from(DocListTool {
            store: state.store.clone(),
        }),
        ToolRegistration::from(DocOpenTool {
            store: state.store.clone(),
        }),
        ToolRegistration::from(DocDeleteTool {
            store: state.store.clone(),
        }),
        ToolRegistration::from(DocOutlineTool {
            store: state.store.clone(),
        }),
        ToolRegistration::from(DocMetadataTool {
            store: state.store.clone(),
        }),
    ]
}

// ===========================================================================
// doc_create
// ===========================================================================

#[tool(
    name = "doc_create",
    description = "Create a new writing document (paper, report, thesis chapter). \
                  Returns the document_id. \
                  \
                  **Examples**: \
                  • title=\"Genetic Architecture of BMI\", class=\"article\" \
                  • title=\"My Thesis\", class=\"report\""
)]
pub struct DocCreateInput {
    #[desc = "Document title"]
    pub title: String,
    #[desc = "LaTeX document class: \"article\", \"report\", \"book\", \"beamer\", or a custom class like \"revtex4-2\". Default: \"article\"."]
    pub document_class: Option<String>,
    #[desc = "Optional: author name(s)"]
    pub authors: Option<Vec<String>>,
}

struct DocCreateTool {
    store: Arc<WritingStore>,
}

#[async_trait]
impl ToolFunction for DocCreateTool {
    type Input = DocCreateInput;

    async fn run(&self, input: Self::Input) -> Result<AgentToolResult, ToolError> {
        let class = input.document_class.as_deref().unwrap_or("article");
        let doc_class = match class {
            "article" => DocumentClass::Article,
            "report" => DocumentClass::Report,
            "book" => DocumentClass::Book,
            "beamer" => DocumentClass::Beamer,
            other => DocumentClass::Custom(other.into()),
        };

        let id = format!("doc-{}", short_id());

        let doc = match self.store.create_document(&id, &input.title).await {
            Ok(mut doc) => {
                doc.document_class = doc_class;
                if let Some(authors) = &input.authors {
                    doc.authors = authors
                        .iter()
                        .map(|name| DocumentAuthor {
                            name: name.clone(),
                            ..Default::default()
                        })
                        .collect();
                }
                self.store
                    .save_document(&mut doc, "set class + authors", None)
                    .await
                    .map_err(|e| ToolError::ExecutionFailed {
                        source: Box::new(e),
                    })?;
                doc
            }
            Err(e) => {
                return Err(ToolError::ExecutionFailed {
                    source: Box::new(e),
                });
            }
        };

        Ok(AgentToolResult::success_json(serde_json::json!({
            "document_id": doc.id,
            "title": doc.title,
            "version": doc.version,
            "document_class": class,
        })))
    }
}

// ===========================================================================
// doc_list
// ===========================================================================

#[tool(
    name = "doc_list",
    description = "List all writing documents in the library. Returns id, title, version, and timestamps."
)]
pub struct DocListInput {
    #[desc = "Filter by title substring (optional). Pass empty string or omit to list all."]
    pub filter: Option<String>,
}

struct DocListTool {
    store: Arc<WritingStore>,
}

#[async_trait]
impl ToolFunction for DocListTool {
    type Input = DocListInput;

    async fn run(&self, _input: Self::Input) -> Result<AgentToolResult, ToolError> {
        let docs = self
            .store
            .list_documents()
            .await
            .map_err(|e| ToolError::ExecutionFailed {
                source: Box::new(e),
            })?;

        let entries: Vec<_> = docs
            .iter()
            .map(|d| {
                serde_json::json!({
                    "id": d.id,
                    "title": d.title,
                    "version": d.version,
                    "updated_at": d.updated_at,
                })
            })
            .collect();

        Ok(AgentToolResult::success_json(serde_json::json!({
            "count": entries.len(),
            "documents": entries,
        })))
    }
}

// ===========================================================================
// doc_open
// ===========================================================================

#[tool(
    name = "doc_open",
    description = "Open a document and return its structure: outline (sections + block count), \
                  metadata (title, authors, keywords, abstract), and version. \
                  Does NOT return full block content — use doc_outline for structure or \
                  read specific sections via the section IDs returned here."
)]
pub struct DocOpenInput {
    #[desc = "Document ID"]
    pub document_id: String,
}

struct DocOpenTool {
    store: Arc<WritingStore>,
}

#[async_trait]
impl ToolFunction for DocOpenTool {
    type Input = DocOpenInput;

    async fn run(&self, input: Self::Input) -> Result<AgentToolResult, ToolError> {
        let doc = self
            .store
            .get_document(&input.document_id)
            .await
            .map_err(|e| ToolError::ExecutionFailed {
                source: Box::new(e),
            })?;

        let outline = ast::outline(&doc);

        Ok(AgentToolResult::success_json(serde_json::json!({
            "id": doc.id,
            "title": doc.title,
            "version": doc.version,
            "document_class": doc.document_class.class_name(),
            "authors": doc.authors.iter().map(|a| &a.name).collect::<Vec<_>>(),
            "keywords": doc.metadata.keywords,
            "has_abstract": doc.metadata.abstract_text.is_some(),
            "outline": outline.items.iter().map(|item| {
                serde_json::json!({
                    "section_id": item.section_id,
                    "title": item.title,
                    "level": item.level,
                    "block_count": item.block_count,
                })
            }).collect::<Vec<_>>(),
        })))
    }
}

// ===========================================================================
// doc_delete
// ===========================================================================

#[tool(
    name = "doc_delete",
    description = "Delete a document and all its version history. This cannot be undone."
)]
pub struct DocDeleteInput {
    #[desc = "Document ID to delete"]
    pub document_id: String,
}

struct DocDeleteTool {
    store: Arc<WritingStore>,
}

#[async_trait]
impl ToolFunction for DocDeleteTool {
    type Input = DocDeleteInput;

    async fn run(&self, input: Self::Input) -> Result<AgentToolResult, ToolError> {
        self.store
            .delete_document(&input.document_id)
            .await
            .map_err(|e| ToolError::ExecutionFailed {
                source: Box::new(e),
            })?;

        Ok(AgentToolResult::success_json(serde_json::json!({
            "deleted": true,
            "document_id": input.document_id,
        })))
    }
}

// ===========================================================================
// doc_outline
// ===========================================================================

#[tool(
    name = "doc_outline",
    description = "Get the hierarchical outline of a document (all sections and subsections \
                  with their block counts). Useful for understanding document structure \
                  before editing."
)]
pub struct DocOutlineInput {
    #[desc = "Document ID"]
    pub document_id: String,
}

struct DocOutlineTool {
    store: Arc<WritingStore>,
}

#[async_trait]
impl ToolFunction for DocOutlineTool {
    type Input = DocOutlineInput;

    async fn run(&self, input: Self::Input) -> Result<AgentToolResult, ToolError> {
        let doc = self
            .store
            .get_document(&input.document_id)
            .await
            .map_err(|e| ToolError::ExecutionFailed {
                source: Box::new(e),
            })?;

        let outline = ast::outline(&doc);

        let items: Vec<_> = outline
            .items
            .iter()
            .map(|item| {
                let level_name = match item.level {
                    1 => "part",
                    2 => "chapter",
                    3 => "section",
                    4 => "subsection",
                    5 => "subsubsection",
                    _ => "paragraph",
                };
                serde_json::json!({
                    "section_id": item.section_id,
                    "title": item.title,
                    "level": level_name,
                    "block_count": item.block_count,
                })
            })
            .collect();

        Ok(AgentToolResult::success_json(serde_json::json!({
            "document_id": doc.id,
            "total_sections": items.len(),
            "total_blocks": doc.root.count_blocks(),
            "outline": items,
        })))
    }
}

// ===========================================================================
// doc_metadata
// ===========================================================================

#[tool(
    name = "doc_metadata",
    description = "Get or update document metadata (title, abstract, keywords). \
                  Pass fields to update; omit to just read current values."
)]
pub struct DocMetadataInput {
    #[desc = "Document ID"]
    pub document_id: String,
    #[desc = "New title (optional, only if updating)"]
    pub title: Option<String>,
    #[desc = "New abstract text (optional)"]
    pub abstract_text: Option<String>,
    #[desc = "Keywords to add"]
    pub add_keywords: Option<Vec<String>>,
}

struct DocMetadataTool {
    store: Arc<WritingStore>,
}

#[async_trait]
impl ToolFunction for DocMetadataTool {
    type Input = DocMetadataInput;

    async fn run(&self, input: Self::Input) -> Result<AgentToolResult, ToolError> {
        let mut doc = self
            .store
            .get_document(&input.document_id)
            .await
            .map_err(|e| ToolError::ExecutionFailed {
                source: Box::new(e),
            })?;

        let mut changed = false;

        if let Some(ref title) = input.title {
            doc.title = title.clone();
            changed = true;
        }
        if let Some(ref abs) = input.abstract_text {
            doc.metadata.abstract_text = Some(abs.clone());
            changed = true;
        }
        if let Some(ref keywords) = input.add_keywords {
            for kw in keywords {
                if !doc.metadata.keywords.contains(kw) {
                    doc.metadata.keywords.push(kw.clone());
                }
            }
            changed = true;
        }

        if changed {
            self.store
                .save_document(&mut doc, "update metadata", None)
                .await
                .map_err(|e| ToolError::ExecutionFailed {
                    source: Box::new(e),
                })?;
        }

        Ok(AgentToolResult::success_json(serde_json::json!({
            "document_id": doc.id,
            "title": doc.title,
            "abstract": doc.metadata.abstract_text,
            "keywords": doc.metadata.keywords,
            "version": doc.version,
        })))
    }
}

// ===========================================================================
// Helpers
// ===========================================================================

fn short_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{nanos:x}")[..12].to_string()
}
