//! Edit operations and edit scripts for atomic document editing.
//!
//! Every agent edit command decomposes into a sequence of [`EditOp`]s.
//! Multiple ops can be bundled into an [`EditScript`] for atomic execution.

use serde::{Deserialize, Serialize};

use crate::block::{Block, BlockId};
use crate::claim::Claim;
use crate::document::{DocumentMetadata, Preamble};
use crate::inline::{CitationStyle, Inline};
use crate::section::Section;

// ---------------------------------------------------------------------------
// EditOp
// ---------------------------------------------------------------------------

/// An atomic edit operation on the document tree.
///
/// All operations reference targets by stable [`BlockId`], never by line
/// number.  An [`EditScript`](crate::EditScript) composes multiple ops
/// for atomic application.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum EditOp {
    /// Insert a section.
    InsertSection {
        /// Parent section ID; `None` = root level.
        parent_id: Option<BlockId>,
        /// Insert after this sibling section. `None` = prepend.
        after_section: Option<BlockId>,
        section: Section,
    },

    /// Delete a section and all its descendants.
    DeleteSection {
        section_id: BlockId,
    },

    /// Move a section to a new parent / position.
    MoveSection {
        section_id: BlockId,
        new_parent: Option<BlockId>,
        after_section: Option<BlockId>,
    },

    /// Rename a section heading.
    RenameSection {
        section_id: BlockId,
        new_title: String,
    },

    /// Insert a content block into a section.
    InsertBlock {
        section_id: BlockId,
        /// Insert after this block. `None` = prepend.
        after_block: Option<BlockId>,
        block: Block,
    },

    /// Replace an existing block.
    ReplaceBlock {
        block_id: BlockId,
        new_block: Block,
    },

    /// Delete a block.
    DeleteBlock {
        block_id: BlockId,
    },

    /// Move a block to a different section.
    MoveBlock {
        block_id: BlockId,
        to_section: BlockId,
        after_block: Option<BlockId>,
    },

    /// Append inline content to an existing paragraph.
    AppendToParagraph {
        block_id: BlockId,
        inlines: Vec<Inline>,
    },

    /// Replace all inline content in a paragraph.
    ReplaceParagraph {
        block_id: BlockId,
        inlines: Vec<Inline>,
    },

    /// Insert a citation cluster at a position within a paragraph.
    InsertCitation {
        block_id: BlockId,
        position: CitationPosition,
        keys: Vec<String>,
        style: CitationStyle,
    },

    /// Tag a claim within a block.
    TagClaim {
        block_id: BlockId,
        claim: Claim,
    },

    /// Update document metadata.
    UpdateMetadata {
        changes: MetadataChanges,
    },

    /// Modify the preamble.
    UpdatePreamble {
        changes: PreambleChanges,
    },
}

// ---------------------------------------------------------------------------
// CitationPosition
// ---------------------------------------------------------------------------

/// Where to insert a citation within a paragraph.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CitationPosition {
    /// At the end of the paragraph.
    End,
    /// After the first occurrence of `text`.
    AfterText { text: String },
    /// Before the first occurrence of `text`.
    BeforeText { text: String },
    /// At a specific byte offset in the plain-text rendering.
    AtIndex { index: usize },
}

// ---------------------------------------------------------------------------
// MetadataChanges / PreambleChanges
// ---------------------------------------------------------------------------

/// Changes to apply to [`DocumentMetadata`].
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MetadataChanges {
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub abstract_text: Option<Option<String>>,
    #[serde(default)]
    pub add_keywords: Vec<String>,
    #[serde(default)]
    pub remove_keywords: Vec<String>,
}

/// Changes to apply to [`Preamble`].
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PreambleChanges {
    #[serde(default)]
    pub add_packages: Vec<String>,
    #[serde(default)]
    pub remove_packages: Vec<String>,
    #[serde(default)]
    pub add_custom: Vec<String>,
}

// ---------------------------------------------------------------------------
// EditScript
// ---------------------------------------------------------------------------

/// A sequence of [`EditOp`]s applied atomically.
///
/// If any op fails, the entire script is rolled back and the document is
/// left unchanged.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EditScript {
    pub ops: Vec<EditOp>,
    #[serde(default)]
    pub message: String,
}

impl EditScript {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn op(mut self, op: EditOp) -> Self {
        self.ops.push(op);
        self
    }

    pub fn message(mut self, msg: impl Into<String>) -> Self {
        self.message = msg.into();
        self
    }

    pub fn is_empty(&self) -> bool {
        self.ops.is_empty()
    }

    pub fn len(&self) -> usize {
        self.ops.len()
    }
}

// ---------------------------------------------------------------------------
// Outline
// ---------------------------------------------------------------------------

/// A flat outline representation used for restructuring.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Outline {
    pub items: Vec<OutlineItem>,
}

/// One entry in a document outline.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutlineItem {
    pub section_id: BlockId,
    pub title: String,
    pub level: u8,
    #[serde(default)]
    pub block_count: usize,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::section::SectionLevel;

    #[test]
    fn edit_script_builder() {
        let script = EditScript::new()
            .op(EditOp::InsertSection {
                parent_id: None,
                after_section: None,
                section: Section::new(SectionLevel::Section, "Introduction"),
            })
            .op(EditOp::InsertSection {
                parent_id: None,
                after_section: None,
                section: Section::new(SectionLevel::Section, "Methods"),
            })
            .message("Add Introduction and Methods");

        assert_eq!(script.len(), 2);
        assert_eq!(script.message, "Add Introduction and Methods");
        assert!(!script.is_empty());
    }

    #[test]
    fn citation_position_serialization() {
        let pos = CitationPosition::AfterText {
            text: "shown that".into(),
        };
        let json = serde_json::to_value(&pos).unwrap();
        assert_eq!(json["type"], "after_text");
        assert_eq!(json["text"], "shown that");

        let end = CitationPosition::End;
        let json = serde_json::to_value(&end).unwrap();
        assert_eq!(json["type"], "end");
    }

    #[test]
    fn empty_script_is_empty() {
        let script = EditScript::new();
        assert!(script.is_empty());
        assert_eq!(script.len(), 0);
    }

    #[test]
    fn metadata_changes_serialization() {
        let changes = MetadataChanges {
            title: Some("New Title".into()),
            add_keywords: vec!["genetics".into()],
            ..Default::default()
        };
        let json = serde_json::to_string(&changes).unwrap();
        let back: MetadataChanges = serde_json::from_str(&json).unwrap();
        assert_eq!(back.title.as_deref(), Some("New Title"));
        assert_eq!(back.add_keywords, vec!["genetics"]);
    }
}
