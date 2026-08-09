//! Integration tests for the writing agent tools (Phase 4).
//!
//! These tests exercise the full tool pipeline: create document via tool →
//! edit via tool → check via tool → verify via direct store access.

use std::sync::Arc;

use serde_json::{json, Value};
use writing_base::{
    LatexEngine, NullEngine, WritingStore, ast, writing_all_registrations,
};
use bib_base::BibBase;
use agentik_core::tools::ToolRegistration;

/// Execute a tool by name with the given JSON input.
async fn call_tool(regs: &[ToolRegistration], name: &str, input: Value) -> Value {
    let reg = regs
        .iter()
        .find(|r| r.definition.name == name)
        .unwrap_or_else(|| panic!("tool '{name}' not found"));

    let result = reg
        .implementation
        .execute(input)
        .await
        .unwrap_or_else(|e| panic!("tool '{name}' failed: {e}"));

    // Extract JSON from the result content.
    let content = &result.content;
    match content {
        agentik_sdk::types::ToolResultContent::Json(val) => val.clone(),
        agentik_sdk::types::ToolResultContent::Text(text) => {
            serde_json::from_str(text).unwrap_or_else(|_| json!({ "text": text }))
        }
        _ => json!({}),
    }
}

async fn setup() -> (Arc<WritingStore>, Vec<ToolRegistration>) {
    let store = Arc::new(WritingStore::open_in_memory().await.unwrap());
    let engine: Arc<dyn LatexEngine> = Arc::new(NullEngine);
    let regs = writing_all_registrations(store.clone(), None, Some(engine));
    (store, regs)
}

async fn setup_with_bib() -> (Arc<WritingStore>, Arc<BibBase>, Vec<ToolRegistration>) {
    let store = Arc::new(WritingStore::open_in_memory().await.unwrap());
    let bib = Arc::new(BibBase::open_in_memory().await.unwrap());
    let engine: Arc<dyn LatexEngine> = Arc::new(NullEngine);
    let regs = writing_all_registrations(store.clone(), Some(bib.clone()), Some(engine));
    (store, bib, regs)
}

// ---------------------------------------------------------------------------
// Document management tools
// ---------------------------------------------------------------------------

#[tokio::test]
async fn doc_create_and_list() {
    let (store, regs) = setup().await;

    // Create a document.
    let result = call_tool(&regs, "doc_create", json!({
        "title": "Test Paper",
        "document_class": "article",
    }))
    .await;

    let doc_id = result["document_id"].as_str().unwrap().to_string();
    assert!(!doc_id.is_empty());

    // List documents.
    let list = call_tool(&regs, "doc_list", json!({})).await;
    assert_eq!(list["count"], 1);
    assert_eq!(list["documents"][0]["title"], "Test Paper");

    // Verify via store.
    let doc = store.get_document(&doc_id).await.unwrap();
    assert_eq!(doc.title, "Test Paper");
}

#[tokio::test]
async fn doc_open_and_outline() {
    let (store, regs) = setup().await;

    // Create + add sections via direct editing.
    let mut doc = store.create_document("d1", "Outline Test").await.unwrap();
    use writing_types::*;
    ast::apply_edit(
        &mut doc,
        &EditOp::InsertSection {
            parent_id: None,
            after_section: None,
            section: Section::new(SectionLevel::Section, "Introduction"),
        },
    )
    .unwrap();
    ast::apply_edit(
        &mut doc,
        &EditOp::InsertSection {
            parent_id: None,
            after_section: None,
            section: Section::new(SectionLevel::Section, "Methods"),
        },
    )
    .unwrap();
    store.save_document(&mut doc, "setup", None).await.unwrap();

    // Open via tool.
    let result = call_tool(&regs, "doc_open", json!({
        "document_id": "d1",
    }))
    .await;
    assert_eq!(result["title"], "Outline Test");
    assert_eq!(result["outline"].as_array().unwrap().len(), 2);

    // Outline via tool.
    let outline = call_tool(&regs, "doc_outline", json!({
        "document_id": "d1",
    }))
    .await;
    assert_eq!(outline["total_sections"], 2);
    assert_eq!(outline["outline"][0]["title"], "Introduction");
    assert_eq!(outline["outline"][1]["title"], "Methods");
}

#[tokio::test]
async fn doc_metadata_update() {
    let (store, regs) = setup().await;
    store.create_document("d1", "Original").await.unwrap();

    // Update metadata.
    let result = call_tool(&regs, "doc_metadata", json!({
        "document_id": "d1",
        "title": "Updated Title",
        "abstract_text": "This is the abstract.",
        "add_keywords": ["GWAS", "genetics"],
    }))
    .await;

    assert_eq!(result["title"], "Updated Title");
    assert_eq!(result["abstract"], "This is the abstract.");
    assert!(result["keywords"].as_array().unwrap().len() >= 2);

    // Verify via store.
    let doc = store.get_document("d1").await.unwrap();
    assert_eq!(doc.title, "Updated Title");
}

#[tokio::test]
async fn doc_delete() {
    let (store, regs) = setup().await;
    store.create_document("d1", "ToDelete").await.unwrap();

    call_tool(&regs, "doc_delete", json!({
        "document_id": "d1",
    }))
    .await;

    assert!(store.get_document("d1").await.is_err());
}

// ---------------------------------------------------------------------------
// Editing tools
// ---------------------------------------------------------------------------

#[tokio::test]
async fn doc_insert_section_tool() {
    let (_store, regs) = setup().await;

    // Create doc.
    let create_result = call_tool(&regs, "doc_create", json!({
        "title": "Edit Test",
    }))
    .await;
    let doc_id = create_result["document_id"].as_str().unwrap();

    // Insert section.
    let result = call_tool(&regs, "doc_insert_section", json!({
        "document_id": doc_id,
        "title": "Introduction",
        "level": "section",
        "label": "sec:intro",
    }))
    .await;

    assert!(result["section_id"].as_str().is_some());

    // Verify via outline.
    let outline = call_tool(&regs, "doc_outline", json!({
        "document_id": doc_id,
    }))
    .await;
    assert_eq!(outline["total_sections"], 1);
    assert_eq!(outline["outline"][0]["title"], "Introduction");
}

#[tokio::test]
async fn doc_insert_block_paragraph() {
    let (_store, regs) = setup().await;

    let create = call_tool(&regs, "doc_create", json!({"title": "T"})).await;
    let doc_id = create["document_id"].as_str().unwrap();

    // Insert section.
    let sec = call_tool(&regs, "doc_insert_section", json!({
        "document_id": doc_id,
        "title": "Body",
        "level": "section",
    }))
    .await;
    let section_id = sec["section_id"].as_str().unwrap();

    // Insert paragraph.
    let block = call_tool(&regs, "doc_insert_block", json!({
        "document_id": doc_id,
        "section_id": section_id,
        "block_type": "paragraph",
        "text": "This is a test paragraph.",
    }))
    .await;

    assert!(block["block_id"].as_str().is_some());
    assert_eq!(block["block_type"], "paragraph");
}

#[tokio::test]
async fn doc_insert_block_equation() {
    let (_store, regs) = setup().await;

    let create = call_tool(&regs, "doc_create", json!({"title": "T"})).await;
    let doc_id = create["document_id"].as_str().unwrap();

    let sec = call_tool(&regs, "doc_insert_section", json!({
        "document_id": doc_id, "title": "S", "level": "section",
    }))
    .await;
    let section_id = sec["section_id"].as_str().unwrap();

    let block = call_tool(&regs, "doc_insert_block", json!({
        "document_id": doc_id,
        "section_id": section_id,
        "block_type": "equation",
        "latex": "E = mc^2",
        "label": "eq:energy",
    }))
    .await;

    assert_eq!(block["block_type"], "equation");
}

#[tokio::test]
async fn doc_insert_block_list() {
    let (_store, regs) = setup().await;

    let create = call_tool(&regs, "doc_create", json!({"title": "T"})).await;
    let doc_id = create["document_id"].as_str().unwrap();

    let sec = call_tool(&regs, "doc_insert_section", json!({
        "document_id": doc_id, "title": "S", "level": "section",
    }))
    .await;
    let section_id = sec["section_id"].as_str().unwrap();

    let block = call_tool(&regs, "doc_insert_block", json!({
        "document_id": doc_id,
        "section_id": section_id,
        "block_type": "list",
        "items": ["First", "Second", "Third"],
        "ordered": true,
    }))
    .await;

    assert_eq!(block["block_type"], "list");
}

#[tokio::test]
async fn doc_delete_and_move_block() {
    let (store, regs) = setup().await;

    let create = call_tool(&regs, "doc_create", json!({"title": "T"})).await;
    let doc_id = create["document_id"].as_str().unwrap();

    // Create two sections.
    let sec1 = call_tool(&regs, "doc_insert_section", json!({
        "document_id": doc_id, "title": "A", "level": "section",
    }))
    .await;
    let sec1_id = sec1["section_id"].as_str().unwrap().to_string();

    let sec2 = call_tool(&regs, "doc_insert_section", json!({
        "document_id": doc_id, "title": "B", "level": "section",
    }))
    .await;
    let sec2_id = sec2["section_id"].as_str().unwrap().to_string();

    // Add a block to sec1.
    let block = call_tool(&regs, "doc_insert_block", json!({
        "document_id": doc_id,
        "section_id": &sec1_id,
        "block_type": "paragraph",
        "text": "Move me.",
    }))
    .await;
    let block_id = block["block_id"].as_str().unwrap().to_string();

    // Move block to sec2.
    let moved = call_tool(&regs, "doc_move_block", json!({
        "document_id": doc_id,
        "block_id": &block_id,
        "to_section": &sec2_id,
    }))
    .await;
    assert_eq!(moved["moved"], true);

    // Verify via store.
    let doc = store.get_document(doc_id).await.unwrap();
    let sec1 = doc.root.find(&sec1_id).unwrap();
    let sec2 = doc.root.find(&sec2_id).unwrap();
    assert!(sec1.blocks.is_empty());
    assert_eq!(sec2.blocks.len(), 1);

    // Delete the block.
    call_tool(&regs, "doc_delete_block", json!({
        "document_id": doc_id,
        "block_id": &block_id,
    }))
    .await;

    let doc = store.get_document(doc_id).await.unwrap();
    let sec2 = doc.root.find(&sec2_id).unwrap();
    assert!(sec2.blocks.is_empty());
}

#[tokio::test]
async fn doc_append_and_replace_text() {
    let (_store, regs) = setup().await;

    let create = call_tool(&regs, "doc_create", json!({"title": "T"})).await;
    let doc_id = create["document_id"].as_str().unwrap();

    let sec = call_tool(&regs, "doc_insert_section", json!({
        "document_id": doc_id, "title": "S", "level": "section",
    }))
    .await;
    let section_id = sec["section_id"].as_str().unwrap();

    let block = call_tool(&regs, "doc_insert_block", json!({
        "document_id": doc_id,
        "section_id": section_id,
        "block_type": "paragraph",
        "text": "Original text.",
    }))
    .await;
    let block_id = block["block_id"].as_str().unwrap();

    // Append text.
    call_tool(&regs, "doc_append_text", json!({
        "document_id": doc_id,
        "block_id": block_id,
        "text": " More text.",
    }))
    .await;

    // Replace paragraph.
    let replaced = call_tool(&regs, "doc_replace_paragraph", json!({
        "document_id": doc_id,
        "block_id": block_id,
        "text": "Completely new text.",
    }))
    .await;
    assert_eq!(replaced["replaced"], true);
}

#[tokio::test]
async fn doc_edit_script_multi_op() {
    let (store, regs) = setup().await;

    let create = call_tool(&regs, "doc_create", json!({"title": "T"})).await;
    let doc_id = create["document_id"].as_str().unwrap();

    // Execute a multi-op edit script.
    let result = call_tool(&regs, "doc_edit_script", json!({
        "document_id": doc_id,
        "operations": [
            {"action": "insert_section", "title": "Intro", "level": "section"},
            {"action": "insert_section", "title": "Methods", "level": "section"},
            {"action": "update_metadata", "title": "New Title"},
        ],
        "message": "initial structure",
    }))
    .await;

    assert_eq!(result["applied"], 3);

    // Verify.
    let doc = store.get_document(doc_id).await.unwrap();
    assert_eq!(doc.title, "New Title");
    assert_eq!(doc.root.children.len(), 2);
}

// ---------------------------------------------------------------------------
// Citation tools
// ---------------------------------------------------------------------------

#[tokio::test]
async fn doc_add_citation_tool() {
    use bib_types::{Author, Identifier};

    let (store, bib, regs) = setup_with_bib().await;

    // Add an article to bib.
    let mut a1 = bib_types::Article::new("art-1", "A test paper on genetics");
    a1.authors.push(Author {
        last_name: "TestAuthor".into(),
        fore_name: Some("A".into()),
        initials: Some("A".into()),
        affiliation: None,
        orcid: None,
        corresponding: false,
    });
    a1.year = Some(2024);
    a1.identifiers.push(Identifier::doi("10.1000/test"));
    bib.upsert_article(&a1).await.unwrap();

    // Create doc + section + paragraph.
    let mut doc = store.create_document("d1", "Cite Test").await.unwrap();
    use writing_types::*;
    let mut sec = Section::new(SectionLevel::Section, "Intro");
    let block = Block::Paragraph(ParagraphBlock::text("Prior work was important."));
    let block_id = block.id().to_string();
    sec.blocks.push(block);
    doc.root.children.push(sec);
    store.save_document(&mut doc, "setup", None).await.unwrap();

    // Add citation via tool.
    let result = call_tool(&regs, "doc_add_citation", json!({
        "document_id": "d1",
        "block_id": block_id,
        "keys": ["testauthor2024a"],
        "style": "parenthetical",
        "position": "end",
    }))
    .await;

    assert_eq!(result["added"], 1);
    assert_eq!(result["resolved"], 1);

    // Check citations.
    let check = call_tool(&regs, "doc_check_citations", json!({
        "document_id": "d1",
    }))
    .await;
    assert_eq!(check["resolved"], 1);
    assert_eq!(check["unresolved"], 0);
    assert_eq!(check["all_resolved"], true);
}

#[tokio::test]
async fn doc_generate_bib_tool() {
    use bib_types::{Author, Identifier};

    let (store, bib, regs) = setup_with_bib().await;

    let mut a1 = bib_types::Article::new("art-1", "Another test paper");
    a1.authors.push(Author {
        last_name: "Smith".into(),
        fore_name: Some("J".into()),
        initials: Some("J".into()),
        affiliation: None,
        orcid: None,
        corresponding: false,
    });
    a1.year = Some(2024);
    a1.journal = Some("Nature".into());
    a1.identifiers.push(Identifier::doi("10.1000/smith"));
    bib.upsert_article(&a1).await.unwrap();

    // Create doc with citation.
    let mut doc = store.create_document("d1", "Bib Test").await.unwrap();
    use writing_types::*;
    let mut sec = Section::new(SectionLevel::Section, "S");
    sec.blocks.push(Block::Paragraph(ParagraphBlock::new(vec![
        Inline::text("Cite "),
        Inline::Citation(CitationCluster {
            keys: vec![CiteKey::new("smith2024another")],
            style: CitationStyle::Parenthetical,
            prefix: None,
            suffix: None,
            claim_id: None,
        }),
        Inline::text("."),
    ])));
    doc.root.children.push(sec);
    store.save_document(&mut doc, "setup", None).await.unwrap();

    // Generate bib via tool.
    let result = call_tool(&regs, "doc_generate_bib", json!({
        "document_id": "d1",
    }))
    .await;

    let bib_content = result["bib_content"].as_str().unwrap();
    assert!(bib_content.contains("@article"));
    assert!(bib_content.contains("Smith"));
}

// ---------------------------------------------------------------------------
// Compile + preview tools
// ---------------------------------------------------------------------------

#[tokio::test]
async fn doc_preview_tex_tool() {
    let (_store, regs) = setup().await;

    let create = call_tool(&regs, "doc_create", json!({
        "title": "Preview Test",
    }))
    .await;
    let doc_id = create["document_id"].as_str().unwrap();

    call_tool(&regs, "doc_insert_section", json!({
        "document_id": doc_id, "title": "Intro", "level": "section",
    }))
    .await;

    let result = call_tool(&regs, "doc_preview_tex", json!({
        "document_id": doc_id,
    }))
    .await;

    let tex = result["tex_content"].as_str().unwrap();
    assert!(tex.contains("\\documentclass{article}"));
    assert!(tex.contains("\\section{Intro}"));
    assert!(tex.contains("\\end{document}"));
}

#[tokio::test]
async fn doc_compile_tool_null_engine() {
    let (_store, regs) = setup().await;

    let create = call_tool(&regs, "doc_create", json!({"title": "Compile"})).await;
    let doc_id = create["document_id"].as_str().unwrap();

    call_tool(&regs, "doc_insert_section", json!({
        "document_id": doc_id, "title": "Body", "level": "section",
    }))
    .await;

    let result = call_tool(&regs, "doc_compile", json!({
        "document_id": doc_id,
    }))
    .await;

    // NullEngine always succeeds but doesn't produce PDF.
    assert_eq!(result["success"], true);
    assert_eq!(result["engine"], "null");
}

// ---------------------------------------------------------------------------
// Full agent workflow e2e
// ---------------------------------------------------------------------------

#[tokio::test]
async fn full_agent_workflow() {
    let (_store, regs) = setup().await;

    // 1. Create document.
    let create = call_tool(&regs, "doc_create", json!({
        "title": "Full Workflow Paper",
        "document_class": "article",
        "authors": ["Jane Doe"],
    }))
    .await;
    let doc_id = create["document_id"].as_str().unwrap().to_string();

    // 2. Set metadata.
    call_tool(&regs, "doc_metadata", json!({
        "document_id": &doc_id,
        "abstract_text": "We test the full writing workflow.",
        "add_keywords": ["test", "workflow"],
    }))
    .await;

    // 3. Add sections.
    for title in &["Introduction", "Methods", "Results", "Discussion"] {
        call_tool(&regs, "doc_insert_section", json!({
            "document_id": &doc_id,
            "title": title,
            "level": "section",
        }))
        .await;
    }

    // 4. Get outline.
    let outline = call_tool(&regs, "doc_outline", json!({
        "document_id": &doc_id,
    }))
    .await;
    assert_eq!(outline["total_sections"], 4);

    // 5. Add paragraphs.
    let intro_sec_id = outline["outline"][0]["section_id"].as_str().unwrap();
    call_tool(&regs, "doc_insert_block", json!({
        "document_id": &doc_id,
        "section_id": intro_sec_id,
        "block_type": "paragraph",
        "text": "This is the introduction text.",
    }))
    .await;

    let methods_sec_id = outline["outline"][1]["section_id"].as_str().unwrap();
    call_tool(&regs, "doc_insert_block", json!({
        "document_id": &doc_id,
        "section_id": methods_sec_id,
        "block_type": "equation",
        "latex": "y = X\\beta + \\epsilon",
        "label": "eq:model",
    }))
    .await;

    // 6. Preview tex.
    let preview = call_tool(&regs, "doc_preview_tex", json!({
        "document_id": &doc_id,
    }))
    .await;
    let tex = preview["tex_content"].as_str().unwrap();
    assert!(tex.contains("\\section{Introduction}"));
    assert!(tex.contains("\\section{Methods}"));
    assert!(tex.contains("\\begin{equation}"));
    assert!(tex.contains("y = X\\beta + \\epsilon"));

    // 7. Compile (NullEngine).
    let compile = call_tool(&regs, "doc_compile", json!({
        "document_id": &doc_id,
    }))
    .await;
    assert_eq!(compile["success"], true);
}
