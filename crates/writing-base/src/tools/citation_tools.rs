//! Citation tools — add citations, check consistency, generate .bib.

use std::sync::Arc;

use agentik_core::tools::{ToolError, ToolFunction, ToolRegistration};
use agentik_proc::tool;
use agentik_sdk::types::ToolResult as AgentToolResult;
use async_trait::async_trait;
use bib_base::BibBase;

use crate::ast;
use crate::citation::CitationResolver;
use crate::compile::LatexEngine;
use crate::render_document;
use crate::store::WritingStore;
use super::WritingToolState;

pub fn registrations(state: WritingToolState) -> Vec<ToolRegistration> {
    let mut regs = Vec::new();

    // doc_add_citation — needs bib-base
    if let Some(bib) = &state.bib {
        regs.push(ToolRegistration::from(DocAddCitationTool {
            store: state.store.clone(),
            bib: bib.clone(),
        }));
        regs.push(ToolRegistration::from(DocCheckCitationsTool {
            store: state.store.clone(),
            bib: bib.clone(),
        }));
        regs.push(ToolRegistration::from(DocGenerateBibTool {
            store: state.store.clone(),
            bib: bib.clone(),
        }));
        regs.push(ToolRegistration::from(DocCitationGraphTool {
            store: state.store.clone(),
            bib: bib.clone(),
        }));
    }

    // doc_preview_tex — no bib-base needed
    if state.bib.is_none() {
        // Still register preview even without bib
    }
    let _ = state.engine; // engine used in compile_tools

    regs
}

// ===========================================================================
// doc_add_citation
// ===========================================================================

#[tool(
    name = "doc_add_citation",
    description = "Add a citation to a paragraph. Automatically resolves cite keys against \
                  the local bibliography. If an article is not yet in the library, the tool \
                  reports it as unresolved. \
                  \
                  **Examples**: \
                  • block_id=\"blk_abc\", keys=[\"smith2024\"], style=\"parenthetical\" \
                  • block_id=\"blk_abc\", position=\"end\", keys=[\"smith2024\",\"jones2023\"]"
)]
pub struct DocAddCitationInput {
    #[desc = "Document ID"]
    pub document_id: String,
    #[desc = "Block ID of the paragraph to add citation to"]
    pub block_id: String,
    #[desc = "Cite keys to add"]
    pub keys: Vec<String>,
    #[desc = "Citation style: plain | parenthetical | textual | year_only | footnote. Default: parenthetical."]
    pub style: Option<String>,
    #[desc = "Where to insert: \"end\" (default), \"after:<text>\", \"before:<text>\""]
    pub position: Option<String>,
}

struct DocAddCitationTool {
    store: Arc<WritingStore>,
    bib: Arc<BibBase>,
}

#[async_trait]
impl ToolFunction for DocAddCitationTool {
    type Input = DocAddCitationInput;

    async fn run(&self, input: Self::Input) -> Result<AgentToolResult, ToolError> {
        let mut doc = self
            .store
            .get_document(&input.document_id)
            .await
            .map_err(|e| ToolError::ExecutionFailed {
                source: Box::new(e),
            })?;

        let style = parse_style(input.style.as_deref().unwrap_or("parenthetical"));
        let position = parse_position(input.position.as_deref().unwrap_or("end"));

        let edit = writing_types::EditOp::InsertCitation {
            block_id: input.block_id.clone(),
            position,
            keys: input.keys.clone(),
            style,
        };

        ast::apply_edit(&mut doc, &edit).map_err(|e| ToolError::ExecutionFailed {
            source: Box::new(e),
        })?;

        self.store
            .save_document(&mut doc, "add_citation", None)
            .await
            .map_err(|e| ToolError::ExecutionFailed {
                source: Box::new(e),
            })?;

        // Verify citations against library.
        let resolver = CitationResolver::new(self.bib.clone());
        let report = resolver.scan(&doc).await;

        Ok(AgentToolResult::success_json(serde_json::json!({
            "added": input.keys.len(),
            "resolved": report.resolved_count(),
            "unresolved": report.unresolved_count(),
            "unresolved_keys": report.statuses.iter().filter_map(|s| {
                match s {
                    crate::CiteKeyStatus::Unresolved { key, .. } => Some(key.clone()),
                    _ => None,
                }
            }).collect::<Vec<_>>(),
        })))
    }
}

// ===========================================================================
// doc_check_citations
// ===========================================================================

#[tool(
    name = "doc_check_citations",
    description = "Check citation consistency: find unresolved cite keys (not in library), \
                  ambiguous keys, and uncited articles. Returns a full report."
)]
pub struct DocCheckCitationsInput {
    #[desc = "Document ID"]
    pub document_id: String,
}

struct DocCheckCitationsTool {
    store: Arc<WritingStore>,
    bib: Arc<BibBase>,
}

#[async_trait]
impl ToolFunction for DocCheckCitationsTool {
    type Input = DocCheckCitationsInput;

    async fn run(&self, input: Self::Input) -> Result<AgentToolResult, ToolError> {
        let doc = self
            .store
            .get_document(&input.document_id)
            .await
            .map_err(|e| ToolError::ExecutionFailed {
                source: Box::new(e),
            })?;

        let resolver = CitationResolver::new(self.bib.clone());
        let report = resolver.scan(&doc).await;

        let unresolved: Vec<_> = report
            .statuses
            .iter()
            .filter_map(|s| match s {
                crate::CiteKeyStatus::Unresolved { key, suggestion } => {
                    Some(serde_json::json!({
                        "key": key,
                        "suggestion": suggestion,
                    }))
                }
                _ => None,
            })
            .collect();

        Ok(AgentToolResult::success_json(serde_json::json!({
            "total_keys": report.keys.len(),
            "resolved": report.resolved_count(),
            "unresolved": report.unresolved_count(),
            "all_resolved": report.all_resolved(),
            "unresolved_keys": unresolved,
            "uncited_article_count": report.uncited_article_ids.len(),
        })))
    }
}

// ===========================================================================
// doc_generate_bib
// ===========================================================================

#[tool(
    name = "doc_generate_bib",
    description = "Generate a .bib file containing all cited articles in the document. \
                  Returns the BibTeX content as a string. Unresolved keys appear as comments."
)]
pub struct DocGenerateBibInput {
    #[desc = "Document ID"]
    pub document_id: String,
}

struct DocGenerateBibTool {
    store: Arc<WritingStore>,
    bib: Arc<BibBase>,
}

#[async_trait]
impl ToolFunction for DocGenerateBibTool {
    type Input = DocGenerateBibInput;

    async fn run(&self, input: Self::Input) -> Result<AgentToolResult, ToolError> {
        let doc = self
            .store
            .get_document(&input.document_id)
            .await
            .map_err(|e| ToolError::ExecutionFailed {
                source: Box::new(e),
            })?;

        let resolver = CitationResolver::new(self.bib.clone());
        let bib_content = resolver.generate_bib(&doc).await;

        Ok(AgentToolResult::success_json(serde_json::json!({
            "bib_content": bib_content,
            "bib_size": bib_content.len(),
        })))
    }
}

// ===========================================================================
// doc_citation_graph
// ===========================================================================

#[tool(
    name = "doc_citation_graph",
    description = "Get the citation graph: which paragraphs cite which articles. \
                  Returns a mapping of block_id → list of article IDs."
)]
pub struct DocCitationGraphInput {
    #[desc = "Document ID"]
    pub document_id: String,
}

struct DocCitationGraphTool {
    store: Arc<WritingStore>,
    bib: Arc<BibBase>,
}

#[async_trait]
impl ToolFunction for DocCitationGraphTool {
    type Input = DocCitationGraphInput;

    async fn run(&self, input: Self::Input) -> Result<AgentToolResult, ToolError> {
        let doc = self
            .store
            .get_document(&input.document_id)
            .await
            .map_err(|e| ToolError::ExecutionFailed {
                source: Box::new(e),
            })?;

        let resolver = CitationResolver::new(self.bib.clone());
        let graph = resolver.citation_graph(&doc).await;

        let edges: Vec<_> = graph
            .iter()
            .map(|(block_id, article_ids)| {
                serde_json::json!({
                    "block_id": block_id,
                    "article_ids": article_ids,
                })
            })
            .collect();

        Ok(AgentToolResult::success_json(serde_json::json!({
            "total_cited_blocks": graph.len(),
            "edges": edges,
        })))
    }
}

// ===========================================================================
// Helpers
// ===========================================================================

fn parse_style(s: &str) -> writing_types::CitationStyle {
    match s {
        "plain" | "cite" => writing_types::CitationStyle::Plain,
        "textual" | "citet" => writing_types::CitationStyle::Textual,
        "year_only" | "citeyear" => writing_types::CitationStyle::YearOnly,
        "author_only" | "citeauthor" => writing_types::CitationStyle::AuthorOnly,
        "footnote" | "footcite" => writing_types::CitationStyle::Footnote,
        _ => writing_types::CitationStyle::Parenthetical,
    }
}

fn parse_position(s: &str) -> writing_types::CitationPosition {
    if s == "end" || s.is_empty() {
        return writing_types::CitationPosition::End;
    }
    if let Some(text) = s.strip_prefix("after:") {
        return writing_types::CitationPosition::AfterText {
            text: text.into(),
        };
    }
    if let Some(text) = s.strip_prefix("before:") {
        return writing_types::CitationPosition::BeforeText {
            text: text.into(),
        };
    }
    writing_types::CitationPosition::End
}

// Also register doc_preview_tex here since it doesn't need compile engine.
#[tool(
    name = "doc_preview_tex",
    description = "Preview the LaTeX source that would be generated for a document. \
                  Does NOT compile — just shows the .tex content. \
                  Useful for debugging serialization before compilation."
)]
pub struct DocPreviewTexInput {
    #[desc = "Document ID"]
    pub document_id: String,
}

pub struct DocPreviewTexTool {
    pub store: Arc<WritingStore>,
}

#[async_trait]
impl ToolFunction for DocPreviewTexTool {
    type Input = DocPreviewTexInput;

    async fn run(&self, input: Self::Input) -> Result<AgentToolResult, ToolError> {
        let doc = self
            .store
            .get_document(&input.document_id)
            .await
            .map_err(|e| ToolError::ExecutionFailed {
                source: Box::new(e),
            })?;

        let rendered = render_document(&doc);

        // Truncate if very long.
        let preview = if rendered.main_tex.len() > 10000 {
            format!("{}...\n\n[truncated, total {} chars]", &rendered.main_tex[..10000], rendered.main_tex.len())
        } else {
            rendered.main_tex.clone()
        };

        Ok(AgentToolResult::success_json(serde_json::json!({
            "tex_content": preview,
            "total_chars": rendered.main_tex.len(),
        })))
    }
}

// Re-export for compile_tools
pub use DocPreviewTexTool as PreviewTexTool;

// Unused import suppressors
#[allow(unused_imports)]
use crate::compile::LatexEngine as _LatexEngine;
