//! Section tree — the structural skeleton of a document.

use serde::{Deserialize, Serialize};

use crate::block::{Block, BlockId, BlockMeta};

// ---------------------------------------------------------------------------
// Section
// ---------------------------------------------------------------------------

/// A recursive section node in the document tree.
///
/// Each section owns an ordered list of child sections and an ordered list
/// of [`Block`]s.  The `id` is stable across versions — agent edits locate
/// sections by ID, never by line number.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Section {
    /// Stable identifier (see [`crate::new_block_id`]).
    pub id: BlockId,

    /// Nesting depth — determines the LaTeX command (`\section`, `\subsection`, …).
    pub level: SectionLevel,

    /// Section heading (plain text; formatting is applied during rendering).
    #[serde(default)]
    pub title: String,

    /// `\label{...}` for cross-referencing.
    #[serde(default)]
    pub label: Option<String>,

    /// Child sections, in order.
    #[serde(default)]
    pub children: Vec<Section>,

    /// Content blocks belonging directly to this section.
    #[serde(default)]
    pub blocks: Vec<Block>,

    /// Editorial status — tracks writing progress per section.
    #[serde(default)]
    pub status: SectionStatus,
}

impl Default for Section {
    fn default() -> Self {
        Self::root()
    }
}

impl Section {
    /// Create a new section at the given level.
    pub fn new(level: SectionLevel, title: impl Into<String>) -> Self {
        Self {
            id: crate::new_block_id(),
            level,
            title: title.into(),
            label: None,
            children: Vec::new(),
            blocks: Vec::new(),
            status: SectionStatus::Draft,
        }
    }

    /// Create the implicit root section (not rendered as a heading).
    pub fn root() -> Self {
        Self {
            id: crate::new_block_id(),
            level: SectionLevel::Root,
            title: String::new(),
            label: None,
            children: Vec::new(),
            blocks: Vec::new(),
            status: SectionStatus::Draft,
        }
    }

    /// Find a section (including nested) by ID.
    pub fn find(&self, id: &str) -> Option<&Section> {
        if self.id == id {
            return Some(self);
        }
        self.children.iter().find_map(|c| c.find(id))
    }

    /// Find a section mutably by ID.
    pub fn find_mut(&mut self, id: &str) -> Option<&mut Section> {
        if self.id == id {
            return Some(self);
        }
        self.children.iter_mut().find_map(|c| c.find_mut(id))
    }

    /// Walk all sections (including self) depth-first.
    pub fn walk<F: FnMut(&Section)>(&self, f: &mut F) {
        f(self);
        for child in &self.children {
            child.walk(f);
        }
    }

    /// Count all blocks across all nested sections.
    pub fn count_blocks(&self) -> usize {
        let mut count = self.blocks.len();
        for child in &self.children {
            count += child.count_blocks();
        }
        count
    }
}

// ---------------------------------------------------------------------------
// SectionLevel
// ---------------------------------------------------------------------------

/// Nesting depth of a section heading.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SectionLevel {
    /// Implicit root — never rendered.
    Root,
    Part,
    Chapter,
    Section,
    Subsection,
    Subsubsection,
    /// `\paragraph` (logical, not text-level).
    Paragraph,
    /// `\subparagraph`.
    Subparagraph,
}

impl SectionLevel {
    /// The LaTeX command for this level (empty for `Root`).
    pub fn command(&self) -> &'static str {
        match self {
            Self::Root => "",
            Self::Part => "part",
            Self::Chapter => "chapter",
            Self::Section => "section",
            Self::Subsection => "subsection",
            Self::Subsubsection => "subsubsection",
            Self::Paragraph => "paragraph",
            Self::Subparagraph => "subparagraph",
        }
    }

    /// Numerical depth (for comparison / sorting).
    pub fn depth(&self) -> u8 {
        match self {
            Self::Root => 0,
            Self::Part => 1,
            Self::Chapter => 2,
            Self::Section => 3,
            Self::Subsection => 4,
            Self::Subsubsection => 5,
            Self::Paragraph => 6,
            Self::Subparagraph => 7,
        }
    }
}

// ---------------------------------------------------------------------------
// SectionStatus
// ---------------------------------------------------------------------------

/// Editorial status of a section.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[derive(Default)]
pub enum SectionStatus {
    /// First draft — content being written.
    #[default]
    Draft,
    /// Under active revision.
    Revising,
    /// Ready for / under review.
    Review,
    /// Finalised.
    Final,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn section_find_nested() {
        let mut root = Section::root();
        let mut sec = Section::new(SectionLevel::Section, "Methods");
        let sub = Section::new(SectionLevel::Subsection, "Statistical Analysis");
        let sub_id = sub.id.clone();
        sec.children.push(sub);
        let sec_id = sec.id.clone();
        root.children.push(sec);

        assert!(root.find(&sec_id).is_some());
        assert!(root.find(&sub_id).is_some());
        assert!(root.find("nonexistent").is_none());
    }

    #[test]
    fn section_find_mut() {
        let mut root = Section::root();
        let sec = Section::new(SectionLevel::Section, "Results");
        let id = sec.id.clone();
        root.children.push(sec);

        root.find_mut(&id).unwrap().title = "Updated Results".into();
        assert_eq!(root.find(&id).unwrap().title, "Updated Results");
    }

    #[test]
    fn section_walk_visits_all() {
        let mut root = Section::root();
        let mut a = Section::new(SectionLevel::Section, "A");
        a.children
            .push(Section::new(SectionLevel::Subsection, "A.1"));
        root.children.push(a);
        root.children.push(Section::new(SectionLevel::Section, "B"));

        let mut visited = Vec::new();
        root.walk(&mut |s| visited.push(s.title.clone()));

        // Root + A + A.1 + B = 4
        assert_eq!(visited.len(), 4);
    }

    #[test]
    fn section_level_command() {
        assert_eq!(SectionLevel::Section.command(), "section");
        assert_eq!(SectionLevel::Subsection.command(), "subsection");
        assert_eq!(SectionLevel::Root.command(), "");
    }

    #[test]
    fn section_level_depth_ordering() {
        assert!(SectionLevel::Part.depth() < SectionLevel::Section.depth());
        assert!(SectionLevel::Section.depth() < SectionLevel::Subsection.depth());
    }

    #[test]
    fn json_round_trip() {
        let mut sec = Section::new(SectionLevel::Section, "Introduction");
        sec.label = Some("sec:intro".into());
        sec.status = SectionStatus::Final;

        let json = serde_json::to_string(&sec).unwrap();
        let back: Section = serde_json::from_str(&json).unwrap();

        assert_eq!(back.title, "Introduction");
        assert_eq!(back.label.as_deref(), Some("sec:intro"));
        assert_eq!(back.status, SectionStatus::Final);
    }
}
