//! AST operations — find, insert, delete, move, and apply [`EditOp`] sequences.
//!
//! All operations work on the [`Document`](writing_types::Document) tree in
//! memory.  The store layer persists snapshots; this module is pure logic.
//!
//! ## Atomic edits
//!
//! [`apply_edit_script`] clones the document before applying.  If any op
//! fails the clone is discarded and the original is returned untouched.

use writing_types::{
    CitationCluster, CitationPosition, CitationStyle, CiteKey, Document, EditOp, EditScript,
    Inline, Preamble, PreambleChanges, Section, SectionLevel,
};

use crate::{Error, Result};

// ===========================================================================
// Find helpers
// ===========================================================================

/// Find a section by ID anywhere in the tree.
pub fn find_section<'a>(doc: &'a Document, id: &str) -> Option<&'a Section> {
    doc.root.find(id)
}

/// Find a section by ID, mutably.
pub fn find_section_mut<'a>(doc: &'a mut Document, id: &str) -> Option<&'a mut Section> {
    doc.root.find_mut(id)
}

/// Find the section that owns a given block.
pub fn find_section_of_block<'a>(doc: &'a Document, block_id: &str) -> Option<&'a Section> {
    let mut result = None;
    doc.root.walk(&mut |s| {
        if result.is_some() {
            return;
        }
        if s.blocks.iter().any(|b| b.id() == block_id) {
            result = Some(());
        }
    });
    // walk doesn't return the section; redo with explicit search
    find_section_of_block_recursive(&doc.root, block_id)
}

fn find_section_of_block_recursive<'a>(
    section: &'a Section,
    block_id: &str,
) -> Option<&'a Section> {
    if section.blocks.iter().any(|b| b.id() == block_id) {
        return Some(section);
    }
    for child in &section.children {
        if let Some(s) = find_section_of_block_recursive(child, block_id) {
            return Some(s);
        }
    }
    None
}

/// Collect all blocks from all sections (flattened).
pub fn all_blocks(doc: &Document) -> Vec<&writing_types::Block> {
    let mut blocks = Vec::new();
    collect_blocks(&doc.root, &mut blocks);
    blocks
}

fn collect_blocks<'a>(section: &'a Section, out: &mut Vec<&'a writing_types::Block>) {
    for b in &section.blocks {
        out.push(b);
    }
    for child in &section.children {
        collect_blocks(child, out);
    }
}

/// Collect all cite keys referenced anywhere in the document.
pub fn all_cite_keys(doc: &Document) -> Vec<String> {
    let mut keys = std::collections::BTreeSet::new();
    collect_cite_keys(&doc.root, &mut keys);
    keys.into_iter().collect()
}

fn collect_cite_keys(section: &Section, out: &mut std::collections::BTreeSet<String>) {
    for block in &section.blocks {
        if let writing_types::Block::Paragraph(p) = block {
            for inline in &p.inlines {
                if let Inline::Citation(cluster) = inline {
                    for key in &cluster.keys {
                        out.insert(key.key.clone());
                    }
                }
            }
        }
    }
    for child in &section.children {
        collect_cite_keys(child, out);
    }
}

// ===========================================================================
// Edit application
// ===========================================================================

/// Apply a single [`EditOp`] to the document.
///
/// Mutates `doc` in place.  On error the document may be partially modified;
/// use [`apply_edit_script`] for atomic (all-or-nothing) semantics.
pub fn apply_edit(doc: &mut Document, op: &EditOp) -> Result<()> {
    match op {
        EditOp::InsertSection {
            parent_id,
            after_section,
            section,
        } => {
            let parent = match parent_id {
                Some(pid) => doc
                    .root
                    .find_mut(pid)
                    .ok_or_else(|| Error::Edit(format!("parent section not found: {pid}")))?,
                None => &mut doc.root,
            };
            insert_section_after(parent, after_section.as_deref(), section.clone());
            Ok(())
        }

        EditOp::DeleteSection { section_id } => delete_section_recursive(&mut doc.root, section_id)
            .then_some(())
            .ok_or_else(|| Error::Edit(format!("section not found: {section_id}"))),

        EditOp::MoveSection {
            section_id,
            new_parent,
            after_section,
        } => {
            // Remove from current location.
            let section = remove_section_recursive(&mut doc.root, section_id)
                .ok_or_else(|| Error::Edit(format!("section not found: {section_id}")))?;

            // Insert at new location.
            let parent = match new_parent {
                Some(pid) => doc
                    .root
                    .find_mut(pid)
                    .ok_or_else(|| Error::Edit(format!("new parent not found: {pid}")))?,
                None => &mut doc.root,
            };
            insert_section_after(parent, after_section.as_deref(), section);
            Ok(())
        }

        EditOp::RenameSection {
            section_id,
            new_title,
        } => {
            let sec = doc
                .root
                .find_mut(section_id)
                .ok_or_else(|| Error::Edit(format!("section not found: {section_id}")))?;
            sec.title = new_title.clone();
            Ok(())
        }

        EditOp::InsertBlock {
            section_id,
            after_block,
            block,
        } => {
            let sec = doc
                .root
                .find_mut(section_id)
                .ok_or_else(|| Error::Edit(format!("section not found: {section_id}")))?;
            insert_block_after(sec, after_block.as_deref(), block.clone());
            Ok(())
        }

        EditOp::ReplaceBlock {
            block_id,
            new_block,
        } => {
            let (sec, idx) = find_block_loc(&mut doc.root, block_id)
                .ok_or_else(|| Error::Edit(format!("block not found: {block_id}")))?;
            sec.blocks[idx] = new_block.clone();
            Ok(())
        }

        EditOp::DeleteBlock { block_id } => delete_block_recursive(&mut doc.root, block_id)
            .then_some(())
            .ok_or_else(|| Error::Edit(format!("block not found: {block_id}"))),

        EditOp::MoveBlock {
            block_id,
            to_section,
            after_block,
        } => {
            // Remove from current location.
            let block = remove_block_recursive(&mut doc.root, block_id)
                .ok_or_else(|| Error::Edit(format!("block not found: {block_id}")))?;

            // Insert at destination.
            let dest = doc.root.find_mut(to_section).ok_or_else(|| {
                Error::Edit(format!("destination section not found: {to_section}"))
            })?;
            insert_block_after(dest, after_block.as_deref(), block);
            Ok(())
        }

        EditOp::AppendToParagraph { block_id, inlines } => {
            let para = find_paragraph_mut(&mut doc.root, block_id)
                .ok_or_else(|| Error::Edit(format!("paragraph not found: {block_id}")))?;
            para.inlines.extend(inlines.iter().cloned());
            Ok(())
        }

        EditOp::ReplaceParagraph { block_id, inlines } => {
            let para = find_paragraph_mut(&mut doc.root, block_id)
                .ok_or_else(|| Error::Edit(format!("paragraph not found: {block_id}")))?;
            para.inlines = inlines.clone();
            Ok(())
        }

        EditOp::InsertCitation {
            block_id,
            position,
            keys,
            style,
        } => {
            let para = find_paragraph_mut(&mut doc.root, block_id)
                .ok_or_else(|| Error::Edit(format!("paragraph not found: {block_id}")))?;

            let cluster = CitationCluster {
                keys: keys.iter().map(|k| CiteKey::new(k.clone())).collect(),
                style: *style,
                prefix: None,
                suffix: None,
                claim_id: None,
            };
            let citation = Inline::Citation(cluster);

            match position {
                CitationPosition::End => {
                    para.inlines.push(citation);
                }
                CitationPosition::AtIndex { index } => {
                    let idx = (*index).min(para.inlines.len());
                    para.inlines.insert(idx, citation);
                }
                CitationPosition::AfterText { text } => {
                    let plain = para.plain_text();
                    if let Some(pos) = plain.find(text) {
                        let insert_idx =
                            find_inline_index_after_byte(&para.inlines, pos + text.len());
                        para.inlines.insert(insert_idx, citation);
                    } else {
                        return Err(Error::Edit(format!(
                            "text '{text}' not found in block {block_id}"
                        )));
                    }
                }
                CitationPosition::BeforeText { text } => {
                    let plain = para.plain_text();
                    if let Some(pos) = plain.find(text) {
                        let insert_idx = find_inline_index_after_byte(&para.inlines, pos);
                        para.inlines.insert(insert_idx, citation);
                    } else {
                        return Err(Error::Edit(format!(
                            "text '{text}' not found in block {block_id}"
                        )));
                    }
                }
            }
            Ok(())
        }

        EditOp::TagClaim { block_id, claim } => {
            // Claims are stored in the block's tags for now.  The full claim
            // object is stored elsewhere in the persistence layer.
            let sec_idx = find_block_loc(&mut doc.root, block_id)
                .ok_or_else(|| Error::Edit(format!("block not found: {block_id}")))?;
            let tag = format!("claim:{}", claim.id);
            sec_idx.0.blocks[sec_idx.1].add_claim_tag(&tag);
            Ok(())
        }

        EditOp::UpdateMetadata { changes } => {
            if let Some(ref title) = changes.title {
                doc.title = title.clone();
            }
            if let Some(ref abs) = changes.abstract_text {
                doc.metadata.abstract_text = abs.clone();
            }
            for kw in &changes.add_keywords {
                if !doc.metadata.keywords.contains(kw) {
                    doc.metadata.keywords.push(kw.clone());
                }
            }
            doc.metadata
                .keywords
                .retain(|k| !changes.remove_keywords.contains(k));
            Ok(())
        }

        EditOp::UpdatePreamble { changes } => {
            for pkg in &changes.add_packages {
                if !doc.preamble.packages.contains(pkg) {
                    doc.preamble.packages.push(pkg.clone());
                }
            }
            doc.preamble
                .packages
                .retain(|p| !changes.remove_packages.contains(p));
            doc.preamble
                .custom
                .extend(changes.add_custom.iter().cloned());
            Ok(())
        }
    }
}

/// Apply an [`EditScript`] atomically — all ops succeed or none take effect.
///
/// This clones the document before applying.  On success the original is
/// mutated; on failure it is left untouched.
pub fn apply_edit_script(doc: &mut Document, script: &EditScript) -> Result<()> {
    if script.ops.is_empty() {
        return Ok(());
    }

    // Clone, apply to clone, and only commit on success.
    let mut working = doc.clone();
    for (i, op) in script.ops.iter().enumerate() {
        if let Err(e) = apply_edit(&mut working, op) {
            return Err(Error::Edit(format!(
                "edit op #{i} failed ({}): {e}; script rolled back",
                op_name(op)
            )));
        }
    }
    *doc = working;
    doc.version += 1;
    Ok(())
}

fn op_name(op: &EditOp) -> &'static str {
    match op {
        EditOp::InsertSection { .. } => "insert_section",
        EditOp::DeleteSection { .. } => "delete_section",
        EditOp::MoveSection { .. } => "move_section",
        EditOp::RenameSection { .. } => "rename_section",
        EditOp::InsertBlock { .. } => "insert_block",
        EditOp::ReplaceBlock { .. } => "replace_block",
        EditOp::DeleteBlock { .. } => "delete_block",
        EditOp::MoveBlock { .. } => "move_block",
        EditOp::AppendToParagraph { .. } => "append_to_paragraph",
        EditOp::ReplaceParagraph { .. } => "replace_paragraph",
        EditOp::InsertCitation { .. } => "insert_citation",
        EditOp::TagClaim { .. } => "tag_claim",
        EditOp::UpdateMetadata { .. } => "update_metadata",
        EditOp::UpdatePreamble { .. } => "update_preamble",
    }
}

// ===========================================================================
// Internal helpers
// ===========================================================================

fn insert_section_after(parent: &mut Section, after: Option<&str>, new_section: Section) {
    match after {
        Some(after_id) => {
            let pos = parent
                .children
                .iter()
                .position(|c| c.id == after_id)
                .map(|p| p + 1)
                .unwrap_or(parent.children.len());
            parent.children.insert(pos, new_section);
        }
        None => parent.children.push(new_section),
    }
}

fn delete_section_recursive(root: &mut Section, id: &str) -> bool {
    let before = root.children.len();
    root.children.retain(|c| c.id != id);
    if root.children.len() < before {
        return true;
    }
    for child in &mut root.children {
        if delete_section_recursive(child, id) {
            return true;
        }
    }
    false
}

fn remove_section_recursive(root: &mut Section, id: &str) -> Option<Section> {
    if let Some(pos) = root.children.iter().position(|c| c.id == id) {
        return Some(root.children.remove(pos));
    }
    for child in &mut root.children {
        if let Some(s) = remove_section_recursive(child, id) {
            return Some(s);
        }
    }
    None
}

fn insert_block_after(section: &mut Section, after: Option<&str>, block: writing_types::Block) {
    match after {
        Some(after_id) => {
            let pos = section
                .blocks
                .iter()
                .position(|b| b.id() == after_id)
                .map(|p| p + 1)
                .unwrap_or(section.blocks.len());
            section.blocks.insert(pos, block);
        }
        None => section.blocks.push(block),
    }
}

fn find_block_loc<'a>(root: &'a mut Section, block_id: &str) -> Option<(&'a mut Section, usize)> {
    if let Some(idx) = root.blocks.iter().position(|b| b.id() == block_id) {
        return Some((root, idx));
    }
    for child in &mut root.children {
        if let Some(loc) = find_block_loc(child, block_id) {
            return Some(loc);
        }
    }
    None
}

fn delete_block_recursive(root: &mut Section, id: &str) -> bool {
    let before = root.blocks.len();
    root.blocks.retain(|b| b.id() != id);
    if root.blocks.len() < before {
        return true;
    }
    for child in &mut root.children {
        if delete_block_recursive(child, id) {
            return true;
        }
    }
    false
}

fn remove_block_recursive(root: &mut Section, id: &str) -> Option<writing_types::Block> {
    if let Some(idx) = root.blocks.iter().position(|b| b.id() == id) {
        return Some(root.blocks.remove(idx));
    }
    for child in &mut root.children {
        if let Some(b) = remove_block_recursive(child, id) {
            return Some(b);
        }
    }
    None
}

fn find_paragraph_mut<'a>(
    root: &'a mut Section,
    block_id: &str,
) -> Option<&'a mut writing_types::ParagraphBlock> {
    for block in &mut root.blocks {
        if let writing_types::Block::Paragraph(p) = block {
            if p.meta.id == block_id {
                return Some(p);
            }
        }
    }
    for child in &mut root.children {
        if let Some(p) = find_paragraph_mut(child, block_id) {
            return Some(p);
        }
    }
    None
}

/// Find the inline-element index that contains (or follows) a given byte
/// offset in the plain-text rendering.
fn find_inline_index_after_byte(inlines: &[Inline], byte_offset: usize) -> usize {
    let mut acc = 0;
    for (i, inline) in inlines.iter().enumerate() {
        let len = inline.plain_text().len();
        if acc + len >= byte_offset {
            return i;
        }
        acc += len;
    }
    inlines.len()
}

/// Helper trait to add a claim tag to any block.
trait ClaimTag {
    fn add_claim_tag(&mut self, tag: &str);
}

impl ClaimTag for writing_types::Block {
    fn add_claim_tag(&mut self, tag: &str) {
        match self {
            writing_types::Block::Paragraph(b) => {
                if !b.meta.tags.contains(&tag.to_string()) {
                    b.meta.tags.push(tag.to_string());
                }
            }
            writing_types::Block::Equation(b) => {
                if !b.meta.tags.contains(&tag.to_string()) {
                    b.meta.tags.push(tag.to_string());
                }
            }
            writing_types::Block::Figure(b) => {
                if !b.meta.tags.contains(&tag.to_string()) {
                    b.meta.tags.push(tag.to_string());
                }
            }
            writing_types::Block::Table(b) => {
                if !b.meta.tags.contains(&tag.to_string()) {
                    b.meta.tags.push(tag.to_string());
                }
            }
            _ => {}
        }
    }
}

// ===========================================================================
// Document outline
// ===========================================================================

/// Build a flat outline of the document (for display / validation).
pub fn outline(doc: &Document) -> writing_types::Outline {
    let mut items = Vec::new();
    outline_recursive(&doc.root, SectionLevel::Root, &mut items);
    writing_types::Outline { items }
}

fn outline_recursive(
    section: &Section,
    level: SectionLevel,
    out: &mut Vec<writing_types::OutlineItem>,
) {
    if level != SectionLevel::Root {
        out.push(writing_types::OutlineItem {
            section_id: section.id.clone(),
            title: section.title.clone(),
            level: level.depth(),
            block_count: section.blocks.len(),
        });
    }
    for child in &section.children {
        outline_recursive(child, child.level, out);
    }
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use writing_types::{
        Block, BlockMeta, EditScript, EquationBlock, ParagraphBlock, Placement, Section,
        SectionLevel, SectionStatus, TableBlock, TableFormat, TableSource,
    };

    fn make_doc() -> Document {
        let mut doc = Document::new("d1", "Test Paper");
        let mut intro = Section::new(SectionLevel::Section, "Introduction");
        intro.blocks.push(Block::Paragraph(ParagraphBlock::text(
            "This is the introduction.",
        )));
        intro.blocks.push(Block::Paragraph(ParagraphBlock::text(
            "It has two paragraphs.",
        )));
        doc.root.children.push(intro);

        let methods = Section::new(SectionLevel::Section, "Methods");
        doc.root.children.push(methods);

        doc
    }

    #[test]
    fn find_section_by_id() {
        let doc = make_doc();
        let intro = &doc.root.children[0];
        assert!(find_section(&doc, &intro.id).is_some());
        assert!(find_section(&doc, "nonexistent").is_none());
    }

    #[test]
    fn test_find_section_of_block() {
        let doc = make_doc();
        let intro = &doc.root.children[0];
        let block_id = intro.blocks[0].id().to_string();
        let owner = super::find_section_of_block(&doc, &block_id).unwrap();
        assert_eq!(owner.title, "Introduction");
    }

    #[test]
    fn all_blocks_collected() {
        let doc = make_doc();
        let blocks = all_blocks(&doc);
        assert_eq!(blocks.len(), 2);
    }

    #[test]
    fn all_cite_keys_collected() {
        let mut doc = make_doc();
        let block_id = doc.root.children[0].blocks[0].id().to_string();
        let para = find_paragraph_mut(&mut doc.root, &block_id).unwrap();
        para.push(Inline::Citation(CitationCluster {
            keys: vec![CiteKey::new("smith2024"), CiteKey::new("jones2023")],
            style: CitationStyle::Parenthetical,
            prefix: None,
            suffix: None,
            claim_id: None,
        }));

        let keys = all_cite_keys(&doc);
        assert_eq!(keys, vec!["jones2023", "smith2024"]);
    }

    #[test]
    fn apply_insert_section() {
        let mut doc = make_doc();
        let new_sec = Section::new(SectionLevel::Section, "Results");

        let op = EditOp::InsertSection {
            parent_id: None,
            after_section: Some(doc.root.children[0].id.clone()),
            section: new_sec.clone(),
        };

        apply_edit(&mut doc, &op).unwrap();
        assert_eq!(doc.root.children.len(), 3);
        assert_eq!(doc.root.children[1].title, "Results");
    }

    #[test]
    fn apply_delete_section() {
        let mut doc = make_doc();
        let methods_id = doc.root.children[1].id.clone();

        let op = EditOp::DeleteSection {
            section_id: methods_id,
        };
        apply_edit(&mut doc, &op).unwrap();
        assert_eq!(doc.root.children.len(), 1);
        assert_eq!(doc.root.children[0].title, "Introduction");
    }

    #[test]
    fn apply_move_section() {
        let mut doc = make_doc();
        let intro_id = doc.root.children[0].id.clone();

        // Move Introduction after Methods.
        let op = EditOp::MoveSection {
            section_id: intro_id.clone(),
            new_parent: None,
            after_section: Some(doc.root.children[1].id.clone()),
        };
        apply_edit(&mut doc, &op).unwrap();
        assert_eq!(doc.root.children[0].title, "Methods");
        assert_eq!(doc.root.children[1].title, "Introduction");
    }

    #[test]
    fn apply_rename_section() {
        let mut doc = make_doc();
        let intro_id = doc.root.children[0].id.clone();

        let op = EditOp::RenameSection {
            section_id: intro_id,
            new_title: "Background".into(),
        };
        apply_edit(&mut doc, &op).unwrap();
        assert_eq!(doc.root.children[0].title, "Background");
    }

    #[test]
    fn apply_insert_block() {
        let mut doc = make_doc();
        let methods_id = doc.root.children[1].id.clone();

        let eq = Block::Equation(EquationBlock {
            meta: BlockMeta::new(),
            latex: "y = \\beta_0 + \\beta_1 x".into(),
            numbered: true,
        });

        let op = EditOp::InsertBlock {
            section_id: methods_id,
            after_block: None,
            block: eq,
        };
        apply_edit(&mut doc, &op).unwrap();
        assert_eq!(doc.root.children[1].blocks.len(), 1);
        assert_eq!(doc.root.children[1].blocks[0].kind_name(), "equation");
    }

    #[test]
    fn apply_delete_block() {
        let mut doc = make_doc();
        let block_id = doc.root.children[0].blocks[0].id().to_string();

        let op = EditOp::DeleteBlock {
            block_id: block_id.clone(),
        };
        apply_edit(&mut doc, &op).unwrap();
        assert_eq!(doc.root.children[0].blocks.len(), 1);
    }

    #[test]
    fn apply_move_block() {
        let mut doc = make_doc();
        let block_id = doc.root.children[0].blocks[0].id().to_string();
        let methods_id = doc.root.children[1].id.clone();

        let op = EditOp::MoveBlock {
            block_id,
            to_section: methods_id,
            after_block: None,
        };
        apply_edit(&mut doc, &op).unwrap();
        assert_eq!(doc.root.children[0].blocks.len(), 1);
        assert_eq!(doc.root.children[1].blocks.len(), 1);
    }

    #[test]
    fn apply_append_to_paragraph() {
        let mut doc = make_doc();
        let block_id = doc.root.children[0].blocks[0].id().to_string();

        let op = EditOp::AppendToParagraph {
            block_id,
            inlines: vec![Inline::text(" More text.")],
        };
        apply_edit(&mut doc, &op).unwrap();

        let bid = doc.root.children[0].blocks[0].id().to_string();
        let para = find_paragraph_mut(&mut doc.root, &bid).unwrap();
        assert!(para.plain_text().ends_with(" More text."));
    }

    #[test]
    fn apply_replace_paragraph() {
        let mut doc = make_doc();
        let block_id = doc.root.children[0].blocks[0].id().to_string();

        let op = EditOp::ReplaceParagraph {
            block_id,
            inlines: vec![Inline::text("Replacement text.")],
        };
        apply_edit(&mut doc, &op).unwrap();

        let bid = doc.root.children[0].blocks[0].id().to_string();
        let para = find_paragraph_mut(&mut doc.root, &bid).unwrap();
        assert_eq!(para.plain_text(), "Replacement text.");
    }

    #[test]
    fn apply_insert_citation_end() {
        let mut doc = make_doc();
        let block_id = doc.root.children[0].blocks[0].id().to_string();

        let op = EditOp::InsertCitation {
            block_id,
            position: CitationPosition::End,
            keys: vec!["smith2024".into()],
            style: CitationStyle::Parenthetical,
        };
        apply_edit(&mut doc, &op).unwrap();

        let keys = all_cite_keys(&doc);
        assert_eq!(keys, vec!["smith2024"]);
    }

    #[test]
    fn apply_insert_citation_after_text() {
        let mut doc = make_doc();
        let block_id = doc.root.children[0].blocks[0].id().to_string();

        let op = EditOp::InsertCitation {
            block_id,
            position: CitationPosition::AfterText {
                text: "introduction".into(),
            },
            keys: vec!["smith2024".into()],
            style: CitationStyle::Parenthetical,
        };
        apply_edit(&mut doc, &op).unwrap();

        let keys = all_cite_keys(&doc);
        assert_eq!(keys, vec!["smith2024"]);
    }

    #[test]
    fn apply_insert_citation_text_not_found() {
        let mut doc = make_doc();
        let block_id = doc.root.children[0].blocks[0].id().to_string();

        let op = EditOp::InsertCitation {
            block_id,
            position: CitationPosition::AfterText {
                text: "nonexistent".into(),
            },
            keys: vec!["smith2024".into()],
            style: CitationStyle::Parenthetical,
        };
        assert!(apply_edit(&mut doc, &op).is_err());
    }

    #[test]
    fn apply_update_metadata() {
        let mut doc = make_doc();

        let op = EditOp::UpdateMetadata {
            changes: writing_types::MetadataChanges {
                title: Some("New Title".into()),
                add_keywords: vec!["GWAS".into()],
                ..Default::default()
            },
        };
        apply_edit(&mut doc, &op).unwrap();
        assert_eq!(doc.title, "New Title");
        assert_eq!(doc.metadata.keywords, vec!["GWAS"]);
    }

    #[test]
    fn apply_update_preamble() {
        let mut doc = make_doc();

        let op = EditOp::UpdatePreamble {
            changes: writing_types::PreambleChanges {
                add_packages: vec!["amsmath".into(), "graphicx".into()],
                ..Default::default()
            },
        };
        apply_edit(&mut doc, &op).unwrap();
        assert!(doc.preamble.packages.contains(&"amsmath".to_string()));
        assert!(doc.preamble.packages.contains(&"graphicx".to_string()));
    }

    #[test]
    fn edit_script_atomic_success() {
        let mut doc = make_doc();
        let sec_id = doc.root.children[0].id.clone();

        let script = EditScript::new()
            .op(EditOp::RenameSection {
                section_id: sec_id.clone(),
                new_title: "Background".into(),
            })
            .op(EditOp::InsertBlock {
                section_id: sec_id,
                after_block: None,
                block: Block::Paragraph(ParagraphBlock::text("New paragraph.")),
            })
            .message("Rename + add paragraph");

        apply_edit_script(&mut doc, &script).unwrap();
        assert_eq!(doc.root.children[0].title, "Background");
        assert_eq!(doc.root.children[0].blocks.len(), 3); // 2 original + 1
    }

    #[test]
    fn edit_script_atomic_rollback() {
        let mut doc = make_doc();
        let original_title = doc.root.children[0].title.clone();
        let original_block_count = doc.root.children[0].blocks.len();

        // Script: valid rename then invalid insert (nonexistent section).
        let script = EditScript::new()
            .op(EditOp::RenameSection {
                section_id: doc.root.children[0].id.clone(),
                new_title: "Changed".into(),
            })
            .op(EditOp::InsertBlock {
                section_id: "nonexistent".into(),
                after_block: None,
                block: Block::Paragraph(ParagraphBlock::text("...")),
            })
            .message("Should roll back");

        let result = apply_edit_script(&mut doc, &script);
        assert!(result.is_err());

        // Document unchanged.
        assert_eq!(doc.root.children[0].title, original_title);
        assert_eq!(doc.root.children[0].blocks.len(), original_block_count);
    }

    #[test]
    fn outline_generation() {
        let mut doc = make_doc();
        // Add a subsection
        let sub = Section::new(SectionLevel::Subsection, "Data");
        doc.root.children[1].children.push(sub);

        let outline = outline(&doc);
        assert_eq!(outline.items.len(), 3);
        assert_eq!(outline.items[0].title, "Introduction");
        assert_eq!(outline.items[1].title, "Methods");
        assert_eq!(outline.items[2].title, "Data");
        // Introduction and Methods are both sections (same depth),
        // but Data is a subsection (deeper).
        assert_eq!(outline.items[0].level, outline.items[1].level);
        assert!(outline.items[2].level > outline.items[1].level);
    }

    #[test]
    fn nested_section_operations() {
        let mut doc = Document::new("d1", "Nested");
        let mut chap1 = Section::new(SectionLevel::Section, "Chapter 1");
        let sub1 = Section::new(SectionLevel::Subsection, "1.1 Background");
        let sub1_id = sub1.id.clone();
        chap1.children.push(sub1);
        let chap1_id = chap1.id.clone();
        doc.root.children.push(chap1);

        // Find nested.
        assert!(find_section(&doc, &sub1_id).is_some());

        // Delete nested.
        apply_edit(
            &mut doc,
            &EditOp::DeleteSection {
                section_id: sub1_id,
            },
        )
        .unwrap();
        assert!(find_section(&doc, &chap1_id).unwrap().children.is_empty());
    }
}
