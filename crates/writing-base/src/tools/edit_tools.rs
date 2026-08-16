//! Editing tools — insert/delete/move sections and blocks, edit scripts.

use std::sync::Arc;

use agentik_core::tools::{ToolError, ToolFunction, ToolRegistration};
use agentik_proc::tool;
use agentik_sdk::types::ToolResult as AgentToolResult;
use async_trait::async_trait;
use writing_types::*;

use super::WritingToolState;
use crate::ast;
use crate::store::WritingStore;

pub fn registrations(state: WritingToolState) -> Vec<ToolRegistration> {
    vec![
        ToolRegistration::from(DocInsertSectionTool {
            store: state.store.clone(),
        }),
        ToolRegistration::from(DocInsertBlockTool {
            store: state.store.clone(),
        }),
        ToolRegistration::from(DocDeleteBlockTool {
            store: state.store.clone(),
        }),
        ToolRegistration::from(DocMoveBlockTool {
            store: state.store.clone(),
        }),
        ToolRegistration::from(DocAppendTextTool {
            store: state.store.clone(),
        }),
        ToolRegistration::from(DocReplaceParagraphTool {
            store: state.store.clone(),
        }),
        ToolRegistration::from(DocEditScriptTool { store: state.store }),
    ]
}

// ===========================================================================
// doc_insert_section
// ===========================================================================

#[tool(
    name = "doc_insert_section",
    description = "Insert a new section into a document. \
                  \
                  **Examples**: \
                  • title=\"Introduction\", level=\"section\" \
                  • title=\"Statistical Analysis\", level=\"subsection\", parent_id=\"sec_methods\" \
                  • after_section=\"sec_intro\" to insert after a specific sibling"
)]
pub struct DocInsertSectionInput {
    #[desc = "Document ID"]
    pub document_id: String,
    #[desc = "Section title"]
    pub title: String,
    #[desc = "Section level: \"section\", \"subsection\", \"subsubsection\", \"chapter\", \"part\". Default: \"section\"."]
    pub level: Option<String>,
    #[desc = "Parent section ID (for nested sections). Omit for top-level."]
    pub parent_id: Option<String>,
    #[desc = "Insert after this sibling section ID. Omit to append to end."]
    pub after_section: Option<String>,
    #[desc = "Optional \\label{...} for cross-referencing"]
    pub label: Option<String>,
}

struct DocInsertSectionTool {
    store: Arc<WritingStore>,
}

#[async_trait]
impl ToolFunction for DocInsertSectionTool {
    type Input = DocInsertSectionInput;

    async fn run(&self, input: Self::Input) -> Result<AgentToolResult, ToolError> {
        let mut doc = load_doc(&self.store, &input.document_id).await?;

        let level = parse_level(input.level.as_deref().unwrap_or("section"));
        let mut section = Section::new(level, &input.title);
        if let Some(ref label) = input.label {
            section.label = Some(label.clone());
        }
        let section_id = section.id.clone();

        let edit = EditOp::InsertSection {
            parent_id: input.parent_id.clone(),
            after_section: input.after_section.clone(),
            section,
        };

        ast::apply_edit(&mut doc, &edit).map_err(tool_err)?;
        save_doc(&self.store, &mut doc, "insert_section").await?;

        Ok(AgentToolResult::success_json(serde_json::json!({
            "section_id": section_id,
            "title": input.title,
            "level": input.level.unwrap_or_else(|| "section".into()),
        })))
    }
}

// ===========================================================================
// doc_insert_block
// ===========================================================================

#[tool(
    name = "doc_insert_block",
    description = "Insert a content block into a section. Block types: \
                  \"paragraph\" (pass 'text'), \"equation\" (pass 'latex'), \
                  \"list\" (pass 'items' as array of strings, optional 'ordered'=true), \
                  \"quote\" (pass 'text'), \"raw_latex\" (pass 'text' as raw LaTeX). \
                  \
                  **Examples**: \
                  • block_type=\"paragraph\", text=\"This is a paragraph.\" \
                  • block_type=\"equation\", latex=\"E = mc^2\" \
                  • block_type=\"list\", items=[\"First\", \"Second\"]"
)]
pub struct DocInsertBlockInput {
    #[desc = "Document ID"]
    pub document_id: String,
    #[desc = "Target section ID"]
    pub section_id: String,
    #[desc = "Block type: paragraph | equation | list | quote | raw_latex"]
    pub block_type: String,
    #[desc = "Text content (for paragraph, quote, raw_latex)"]
    pub text: Option<String>,
    #[desc = "LaTeX equation body (for equation blocks, without \\begin{equation})"]
    pub latex: Option<String>,
    #[desc = "List items (for list blocks)"]
    pub items: Option<Vec<String>>,
    #[desc = "Use numbered list instead of bullets. Default: false."]
    pub ordered: Option<bool>,
    #[desc = "Insert after this block ID. Omit to append to end of section."]
    pub after_block: Option<String>,
    #[desc = "Optional \\label{...} for this block"]
    pub label: Option<String>,
}

struct DocInsertBlockTool {
    store: Arc<WritingStore>,
}

#[async_trait]
impl ToolFunction for DocInsertBlockTool {
    type Input = DocInsertBlockInput;

    async fn run(&self, input: Self::Input) -> Result<AgentToolResult, ToolError> {
        let mut doc = load_doc(&self.store, &input.document_id).await?;

        let block = build_block(&input)?;
        let block_id = block.id().to_string();

        let edit = EditOp::InsertBlock {
            section_id: input.section_id.clone(),
            after_block: input.after_block.clone(),
            block,
        };

        ast::apply_edit(&mut doc, &edit).map_err(tool_err)?;
        save_doc(&self.store, &mut doc, "insert_block").await?;

        Ok(AgentToolResult::success_json(serde_json::json!({
            "block_id": block_id,
            "block_type": input.block_type,
        })))
    }
}

// ===========================================================================
// doc_delete_block
// ===========================================================================

#[tool(
    name = "doc_delete_block",
    description = "Delete a content block from a document by its block ID."
)]
pub struct DocDeleteBlockInput {
    #[desc = "Document ID"]
    pub document_id: String,
    #[desc = "Block ID to delete"]
    pub block_id: String,
}

struct DocDeleteBlockTool {
    store: Arc<WritingStore>,
}

#[async_trait]
impl ToolFunction for DocDeleteBlockTool {
    type Input = DocDeleteBlockInput;

    async fn run(&self, input: Self::Input) -> Result<AgentToolResult, ToolError> {
        let mut doc = load_doc(&self.store, &input.document_id).await?;

        ast::apply_edit(
            &mut doc,
            &EditOp::DeleteBlock {
                block_id: input.block_id.clone(),
            },
        )
        .map_err(tool_err)?;
        save_doc(&self.store, &mut doc, "delete_block").await?;

        Ok(AgentToolResult::success_json(serde_json::json!({
            "deleted": true,
            "block_id": input.block_id,
        })))
    }
}

// ===========================================================================
// doc_move_block
// ===========================================================================

#[tool(
    name = "doc_move_block",
    description = "Move a block to a different section or position. \
                  The block keeps its content but gets a new location in the document."
)]
pub struct DocMoveBlockInput {
    #[desc = "Document ID"]
    pub document_id: String,
    #[desc = "Block ID to move"]
    pub block_id: String,
    #[desc = "Destination section ID"]
    pub to_section: String,
    #[desc = "Insert after this block in the destination. Omit to append."]
    pub after_block: Option<String>,
}

struct DocMoveBlockTool {
    store: Arc<WritingStore>,
}

#[async_trait]
impl ToolFunction for DocMoveBlockTool {
    type Input = DocMoveBlockInput;

    async fn run(&self, input: Self::Input) -> Result<AgentToolResult, ToolError> {
        let mut doc = load_doc(&self.store, &input.document_id).await?;

        ast::apply_edit(
            &mut doc,
            &EditOp::MoveBlock {
                block_id: input.block_id.clone(),
                to_section: input.to_section.clone(),
                after_block: input.after_block.clone(),
            },
        )
        .map_err(tool_err)?;
        save_doc(&self.store, &mut doc, "move_block").await?;

        Ok(AgentToolResult::success_json(serde_json::json!({
            "moved": true,
            "block_id": input.block_id,
            "to_section": input.to_section,
        })))
    }
}

// ===========================================================================
// doc_append_text
// ===========================================================================

#[tool(
    name = "doc_append_text",
    description = "Append text to the end of an existing paragraph. \
                  Quick way to add sentences without replacing the whole paragraph."
)]
pub struct DocAppendTextInput {
    #[desc = "Document ID"]
    pub document_id: String,
    #[desc = "Block ID of the paragraph to append to"]
    pub block_id: String,
    #[desc = "Text to append"]
    pub text: String,
}

struct DocAppendTextTool {
    store: Arc<WritingStore>,
}

#[async_trait]
impl ToolFunction for DocAppendTextTool {
    type Input = DocAppendTextInput;

    async fn run(&self, input: Self::Input) -> Result<AgentToolResult, ToolError> {
        let mut doc = load_doc(&self.store, &input.document_id).await?;

        ast::apply_edit(
            &mut doc,
            &EditOp::AppendToParagraph {
                block_id: input.block_id.clone(),
                inlines: vec![Inline::text(input.text.clone())],
            },
        )
        .map_err(tool_err)?;
        save_doc(&self.store, &mut doc, "append_text").await?;

        Ok(AgentToolResult::success_json(serde_json::json!({
            "appended": true,
            "block_id": input.block_id,
        })))
    }
}

// ===========================================================================
// doc_replace_paragraph
// ===========================================================================

#[tool(
    name = "doc_replace_paragraph",
    description = "Replace the entire content of a paragraph block with new text."
)]
pub struct DocReplaceParagraphInput {
    #[desc = "Document ID"]
    pub document_id: String,
    #[desc = "Block ID of the paragraph to replace"]
    pub block_id: String,
    #[desc = "New paragraph text"]
    pub text: String,
}

struct DocReplaceParagraphTool {
    store: Arc<WritingStore>,
}

#[async_trait]
impl ToolFunction for DocReplaceParagraphTool {
    type Input = DocReplaceParagraphInput;

    async fn run(&self, input: Self::Input) -> Result<AgentToolResult, ToolError> {
        let mut doc = load_doc(&self.store, &input.document_id).await?;

        ast::apply_edit(
            &mut doc,
            &EditOp::ReplaceParagraph {
                block_id: input.block_id.clone(),
                inlines: vec![Inline::text(input.text.clone())],
            },
        )
        .map_err(tool_err)?;
        save_doc(&self.store, &mut doc, "replace_paragraph").await?;

        Ok(AgentToolResult::success_json(serde_json::json!({
            "replaced": true,
            "block_id": input.block_id,
        })))
    }
}

// ===========================================================================
// doc_edit_script
// ===========================================================================

#[tool(
    name = "doc_edit_script",
    description = "Apply a sequence of edit operations atomically (all succeed or none apply). \
                  Use this for complex multi-step edits like restructuring a document. \
                  Each operation in 'operations' is applied in order. \
                  \
                  Operation format: {\"action\": \"insert_section\", \"title\": \"...\", ...} \
                  Actions: insert_section, delete_section, rename_section, insert_block, \
                  delete_block, move_block, append_to_paragraph, replace_paragraph, \
                  insert_citation, update_metadata."
)]
pub struct DocEditScriptInput {
    #[desc = "Document ID"]
    pub document_id: String,
    #[desc = "JSON array of edit operations"]
    pub operations: serde_json::Value,
    #[desc = "Commit message for this edit"]
    pub message: Option<String>,
}

struct DocEditScriptTool {
    store: Arc<WritingStore>,
}

#[async_trait]
impl ToolFunction for DocEditScriptTool {
    type Input = DocEditScriptInput;

    async fn run(&self, input: Self::Input) -> Result<AgentToolResult, ToolError> {
        let mut doc = load_doc(&self.store, &input.document_id).await?;

        // Parse operations JSON into EditOps.
        let ops_arr = input
            .operations
            .as_array()
            .ok_or_else(|| ToolError::ValidationFailed {
                message: "operations must be a JSON array".into(),
            })?;

        let mut script = EditScript::new();
        for (i, op_json) in ops_arr.iter().enumerate() {
            let op = json_to_edit_op(op_json).map_err(|e| ToolError::ValidationFailed {
                message: format!("operation #{i}: {e}"),
            })?;
            script = script.op(op);
        }
        if let Some(ref msg) = input.message {
            script = script.message(msg.clone());
        }

        let op_count = script.len();
        ast::apply_edit_script(&mut doc, &script).map_err(tool_err)?;
        save_doc(
            &self.store,
            &mut doc,
            input.message.as_deref().unwrap_or("edit_script"),
        )
        .await?;

        Ok(AgentToolResult::success_json(serde_json::json!({
            "applied": op_count,
            "version": doc.version,
        })))
    }
}

// ===========================================================================
// Helpers
// ===========================================================================

async fn load_doc(store: &WritingStore, id: &str) -> Result<Document, ToolError> {
    store
        .get_document(id)
        .await
        .map_err(|e| ToolError::ExecutionFailed {
            source: Box::new(e),
        })
}

async fn save_doc(store: &WritingStore, doc: &mut Document, msg: &str) -> Result<(), ToolError> {
    store
        .save_document(doc, msg, None)
        .await
        .map_err(|e| ToolError::ExecutionFailed {
            source: Box::new(e),
        })
}

fn tool_err(e: crate::Error) -> ToolError {
    ToolError::ExecutionFailed {
        source: Box::new(e),
    }
}

fn parse_level(s: &str) -> SectionLevel {
    match s {
        "part" => SectionLevel::Part,
        "chapter" => SectionLevel::Chapter,
        "section" => SectionLevel::Section,
        "subsection" => SectionLevel::Subsection,
        "subsubsection" => SectionLevel::Subsubsection,
        "paragraph" => SectionLevel::Paragraph,
        _ => SectionLevel::Section,
    }
}

fn build_block(input: &DocInsertBlockInput) -> Result<Block, ToolError> {
    let mut meta = BlockMeta::new();
    if let Some(ref label) = input.label {
        meta.label = Some(label.clone());
    }

    match input.block_type.as_str() {
        "paragraph" => {
            let text = input.text.as_deref().unwrap_or("");
            Ok(Block::Paragraph(ParagraphBlock {
                meta,
                inlines: vec![Inline::text(text)],
            }))
        }
        "equation" => {
            let latex = input
                .latex
                .as_deref()
                .ok_or_else(|| ToolError::ValidationFailed {
                    message: "equation blocks require 'latex' field".into(),
                })?;
            Ok(Block::Equation(EquationBlock {
                meta,
                latex: latex.into(),
                numbered: true,
            }))
        }
        "list" => {
            let items = input.items.as_deref().unwrap_or(&[]);
            let marker = if input.ordered.unwrap_or(false) {
                ListMarker::Numbered
            } else {
                ListMarker::Bullet
            };
            let list_items: Vec<ListItem> = items
                .iter()
                .map(|s| ListItem {
                    term: None,
                    inlines: vec![Inline::text(s.clone())],
                })
                .collect();
            Ok(Block::List(ListBlock {
                meta,
                marker,
                items: list_items,
            }))
        }
        "quote" => {
            let text = input.text.as_deref().unwrap_or("");
            Ok(Block::Quote(QuoteBlock {
                meta,
                inlines: vec![Inline::text(text)],
            }))
        }
        "raw_latex" => {
            let content = input.text.as_deref().unwrap_or("");
            Ok(Block::RawLatex(RawLatexBlock {
                meta,
                content: content.into(),
            }))
        }
        other => Err(ToolError::ValidationFailed {
            message: format!(
                "unknown block type: '{other}'. Valid: paragraph, equation, list, quote, raw_latex"
            ),
        }),
    }
}

/// Convert a JSON object to an EditOp.
fn json_to_edit_op(json: &serde_json::Value) -> Result<EditOp, String> {
    let action = json
        .get("action")
        .and_then(|v| v.as_str())
        .ok_or("missing 'action' field")?;

    match action {
        "insert_section" => {
            let level = json
                .get("level")
                .and_then(|v| v.as_str())
                .unwrap_or("section");
            let mut sec = Section::new(parse_level(level), json_get_str(json, "title")?);
            if let Some(label) = json.get("label").and_then(|v| v.as_str()) {
                sec.label = Some(label.into());
            }
            Ok(EditOp::InsertSection {
                parent_id: json_get_opt_str(json, "parent_id"),
                after_section: json_get_opt_str(json, "after_section"),
                section: sec,
            })
        }
        "delete_section" => Ok(EditOp::DeleteSection {
            section_id: json_get_str(json, "section_id")?,
        }),
        "rename_section" => Ok(EditOp::RenameSection {
            section_id: json_get_str(json, "section_id")?,
            new_title: json_get_str(json, "new_title")?,
        }),
        "insert_block" => {
            let block_type = json_get_str(json, "block_type")?;
            let block = build_block_from_json(&block_type, json)?;
            Ok(EditOp::InsertBlock {
                section_id: json_get_str(json, "section_id")?,
                after_block: json_get_opt_str(json, "after_block"),
                block,
            })
        }
        "delete_block" => Ok(EditOp::DeleteBlock {
            block_id: json_get_str(json, "block_id")?,
        }),
        "move_block" => Ok(EditOp::MoveBlock {
            block_id: json_get_str(json, "block_id")?,
            to_section: json_get_str(json, "to_section")?,
            after_block: json_get_opt_str(json, "after_block"),
        }),
        "append_to_paragraph" => Ok(EditOp::AppendToParagraph {
            block_id: json_get_str(json, "block_id")?,
            inlines: vec![Inline::text(json_get_str(json, "text")?)],
        }),
        "replace_paragraph" => Ok(EditOp::ReplaceParagraph {
            block_id: json_get_str(json, "block_id")?,
            inlines: vec![Inline::text(json_get_str(json, "text")?)],
        }),
        "insert_citation" => {
            let style = json
                .get("style")
                .and_then(|v| v.as_str())
                .unwrap_or("parenthetical");
            let keys: Vec<String> = json
                .get("keys")
                .and_then(|v| v.as_array())
                .ok_or("insert_citation requires 'keys' array")?
                .iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect();
            Ok(EditOp::InsertCitation {
                block_id: json_get_str(json, "block_id")?,
                position: CitationPosition::End,
                keys,
                style: parse_citation_style(style),
            })
        }
        "update_metadata" => Ok(EditOp::UpdateMetadata {
            changes: MetadataChanges {
                title: json.get("title").and_then(|v| v.as_str()).map(String::from),
                ..Default::default()
            },
        }),
        other => Err(format!("unknown action: '{other}'")),
    }
}

fn build_block_from_json(block_type: &str, json: &serde_json::Value) -> Result<Block, String> {
    let meta = BlockMeta::new();
    match block_type {
        "paragraph" => Ok(Block::Paragraph(ParagraphBlock {
            meta,
            inlines: vec![Inline::text(
                json.get("text").and_then(|v| v.as_str()).unwrap_or(""),
            )],
        })),
        "equation" => Ok(Block::Equation(EquationBlock {
            meta,
            latex: json
                .get("latex")
                .and_then(|v| v.as_str())
                .ok_or("equation requires 'latex'")?
                .into(),
            numbered: json
                .get("numbered")
                .and_then(|v| v.as_bool())
                .unwrap_or(true),
        })),
        "raw_latex" => Ok(Block::RawLatex(RawLatexBlock {
            meta,
            content: json
                .get("text")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .into(),
        })),
        "quote" => Ok(Block::Quote(QuoteBlock {
            meta,
            inlines: vec![Inline::text(
                json.get("text").and_then(|v| v.as_str()).unwrap_or(""),
            )],
        })),
        "list" => {
            let ordered = json
                .get("ordered")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let items_arr = json.get("items").and_then(|v| v.as_array());
            let items: Vec<ListItem> = items_arr
                .unwrap_or(serde_json::Value::Array(vec![]).as_array().unwrap())
                .iter()
                .filter_map(|v| v.as_str())
                .map(|s| ListItem {
                    term: None,
                    inlines: vec![Inline::text(s)],
                })
                .collect();
            Ok(Block::List(ListBlock {
                meta,
                marker: if ordered {
                    ListMarker::Numbered
                } else {
                    ListMarker::Bullet
                },
                items,
            }))
        }
        other => Err(format!("unknown block type: '{other}'")),
    }
}

fn parse_citation_style(s: &str) -> CitationStyle {
    match s {
        "plain" | "cite" => CitationStyle::Plain,
        "textual" | "citet" => CitationStyle::Textual,
        "year_only" | "citeyear" => CitationStyle::YearOnly,
        "author_only" | "citeauthor" => CitationStyle::AuthorOnly,
        "footnote" | "footcite" => CitationStyle::Footnote,
        _ => CitationStyle::Parenthetical,
    }
}

fn json_get_str(json: &serde_json::Value, key: &str) -> Result<String, String> {
    json.get(key)
        .and_then(|v| v.as_str())
        .map(String::from)
        .ok_or(format!("missing required field '{key}'"))
}

fn json_get_opt_str(json: &serde_json::Value, key: &str) -> Option<String> {
    json.get(key).and_then(|v| v.as_str()).map(String::from)
}
