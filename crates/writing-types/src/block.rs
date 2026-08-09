//! Content blocks — the structural units within a section.

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// BlockId — re-exported from inline.rs to avoid confusion.
// ---------------------------------------------------------------------------

pub use crate::inline::BlockId;

// ---------------------------------------------------------------------------
// BlockMeta — common metadata for every block variant.
// ---------------------------------------------------------------------------

/// Metadata attached to every [`Block`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlockMeta {
    /// Stable identifier.
    pub id: BlockId,

    /// `\label{...}` for cross-referencing.
    #[serde(default)]
    pub label: Option<String>,

    /// Free-form tags for filtering and organisation.
    #[serde(default)]
    pub tags: Vec<String>,

    /// Inline comments / review notes.
    #[serde(default)]
    pub comments: Vec<BlockComment>,
}

impl BlockMeta {
    pub fn new() -> Self {
        Self {
            id: crate::new_block_id(),
            label: None,
            tags: Vec::new(),
            comments: Vec::new(),
        }
    }

    pub fn with_label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    pub fn with_tags(mut self, tags: Vec<String>) -> Self {
        self.tags = tags;
        self
    }
}

impl Default for BlockMeta {
    fn default() -> Self {
        Self::new()
    }
}

/// A review comment attached to a block.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlockComment {
    pub id: String,
    pub author: String,
    pub text: String,
    #[serde(default)]
    pub resolved: bool,
    #[serde(default)]
    pub created_at: Option<chrono::DateTime<chrono::Utc>>,
}

// ---------------------------------------------------------------------------
// Block enum
// ---------------------------------------------------------------------------

/// A content block — the basic structural unit inside a [`Section`](crate::Section).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Block {
    Paragraph(ParagraphBlock),
    Equation(EquationBlock),
    Figure(crate::figure::FigureBlock),
    Table(crate::figure::TableBlock),
    Code(CodeBlock),
    List(ListBlock),
    Quote(QuoteBlock),
    RawLatex(RawLatexBlock),
    HorizontalRule(HorizontalRule),
    PageBreak(PageBreak),
}

impl Block {
    /// Get the stable ID of this block.
    pub fn id(&self) -> &str {
        match self {
            Self::Paragraph(b) => &b.meta.id,
            Self::Equation(b) => &b.meta.id,
            Self::Figure(b) => &b.meta.id,
            Self::Table(b) => &b.meta.id,
            Self::Code(b) => &b.meta.id,
            Self::List(b) => &b.meta.id,
            Self::Quote(b) => &b.meta.id,
            Self::RawLatex(b) => &b.meta.id,
            Self::HorizontalRule(_) | Self::PageBreak(_) => "structural",
        }
    }

    /// Get the block label (`\label{...}` target).
    pub fn label(&self) -> Option<&str> {
        match self {
            Self::Paragraph(b) => b.meta.label.as_deref(),
            Self::Equation(b) => b.meta.label.as_deref(),
            Self::Figure(b) => b.meta.label.as_deref(),
            Self::Table(b) => b.meta.label.as_deref(),
            Self::Code(b) => b.meta.label.as_deref(),
            Self::List(b) => b.meta.label.as_deref(),
            Self::Quote(b) => b.meta.label.as_deref(),
            Self::RawLatex(b) => b.meta.label.as_deref(),
            Self::HorizontalRule(_) | Self::PageBreak(_) => None,
        }
    }

    /// Get the block's tags.
    pub fn tags(&self) -> &[String] {
        match self {
            Self::Paragraph(b) => &b.meta.tags,
            Self::Equation(b) => &b.meta.tags,
            Self::Figure(b) => &b.meta.tags,
            Self::Table(b) => &b.meta.tags,
            Self::Code(b) => &b.meta.tags,
            Self::List(b) => &b.meta.tags,
            Self::Quote(b) => &b.meta.tags,
            Self::RawLatex(b) => &b.meta.tags,
            Self::HorizontalRule(_) | Self::PageBreak(_) => &[],
        }
    }

    /// Human-readable kind name.
    pub fn kind_name(&self) -> &'static str {
        match self {
            Self::Paragraph(_) => "paragraph",
            Self::Equation(_) => "equation",
            Self::Figure(_) => "figure",
            Self::Table(_) => "table",
            Self::Code(_) => "code",
            Self::List(_) => "list",
            Self::Quote(_) => "quote",
            Self::RawLatex(_) => "raw_latex",
            Self::HorizontalRule(_) => "horizontal_rule",
            Self::PageBreak(_) => "page_break",
        }
    }
}

// ---------------------------------------------------------------------------
// Block variant structs
// ---------------------------------------------------------------------------

/// A paragraph — an ordered sequence of [`Inline`] content.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParagraphBlock {
    pub meta: BlockMeta,
    #[serde(default)]
    pub inlines: Vec<crate::Inline>,
}

impl ParagraphBlock {
    /// Create a paragraph from inline content.
    pub fn new(inlines: Vec<crate::Inline>) -> Self {
        Self {
            meta: BlockMeta::new(),
            inlines,
        }
    }

    /// Create a paragraph from plain text.
    pub fn text(content: impl Into<String>) -> Self {
        Self::new(vec![crate::Inline::text(content)])
    }

    /// Extract the full plain-text rendering of the paragraph.
    pub fn plain_text(&self) -> String {
        self.inlines
            .iter()
            .map(|i| i.plain_text())
            .collect::<Vec<_>>()
            .join("")
    }

    /// Append an inline element to the end of the paragraph.
    pub fn push(&mut self, inline: crate::Inline) {
        self.inlines.push(inline);
    }
}

/// A display equation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EquationBlock {
    pub meta: BlockMeta,
    /// LaTeX body (without `\begin{equation}` / `\end{equation}`).
    pub latex: String,
    /// Whether this is a numbered equation.
    #[serde(default = "default_true")]
    pub numbered: bool,
}

fn default_true() -> bool {
    true
}

/// A code listing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodeBlock {
    pub meta: BlockMeta,
    /// Source code text.
    pub code: String,
    /// Language for syntax highlighting (e.g. `python`, `rust`).
    #[serde(default)]
    pub language: Option<String>,
}

/// A list (itemize / enumerate / description).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListBlock {
    pub meta: BlockMeta,
    pub marker: ListMarker,
    pub items: Vec<ListItem>,
}

/// List bullet style.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ListMarker {
    /// `\begin{itemize}`
    Bullet,
    /// `\begin{enumerate}`
    Numbered,
    /// `\begin{description}`
    Description,
}

/// A single list item.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListItem {
    /// Optional term / label (for description lists).
    #[serde(default)]
    pub term: Option<String>,
    /// Item body (inline content).
    #[serde(default)]
    pub inlines: Vec<crate::Inline>,
}

/// A block quote (`\begin{quote}`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuoteBlock {
    pub meta: BlockMeta,
    #[serde(default)]
    pub inlines: Vec<crate::Inline>,
}

/// Raw LaTeX — passed through verbatim during serialisation.
///
/// This is the escape hatch for content that cannot be expressed in the
/// structured model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawLatexBlock {
    pub meta: BlockMeta,
    /// Raw LaTeX source text.
    pub content: String,
}

/// `\noindent\rule{\linewidth}{0.4pt}` — horizontal rule.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HorizontalRule {
    pub id: BlockId,
}

/// `\newpage`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PageBreak {
    pub id: BlockId,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CitationCluster, CitationStyle, CiteKey, Inline, TextFormat};

    #[test]
    fn paragraph_plain_text() {
        let p = ParagraphBlock::new(vec![
            Inline::text("This is "),
            Inline::formatted("bold", TextFormat::Bold),
            Inline::text(" text."),
        ]);
        assert_eq!(p.plain_text(), "This is bold text.");
    }

    #[test]
    fn paragraph_with_citation() {
        let p = ParagraphBlock::new(vec![
            Inline::text("Prior work "),
            Inline::Citation(CitationCluster {
                keys: vec![CiteKey::new("smith2024")],
                style: CitationStyle::Parenthetical,
                prefix: None,
                suffix: None,
                claim_id: None,
            }),
            Inline::text(" showed this."),
        ]);
        assert_eq!(p.plain_text(), "Prior work [smith2024] showed this.");
    }

    #[test]
    fn block_id_and_label() {
        let p = Block::Paragraph(ParagraphBlock::text("hello"));
        assert!(p.id() != "structural");
        assert_eq!(p.kind_name(), "paragraph");
    }

    #[test]
    fn equation_block() {
        let eq = EquationBlock {
            meta: BlockMeta::new(),
            latex: "E = mc^2".into(),
            numbered: true,
        };
        let json = serde_json::to_string(&eq).unwrap();
        let back: EquationBlock = serde_json::from_str(&json).unwrap();
        assert_eq!(back.latex, "E = mc^2");
        assert!(back.numbered);
    }

    #[test]
    fn raw_latex_round_trip() {
        let raw = RawLatexBlock {
            meta: BlockMeta::new(),
            content: r"\tikz \draw (0,0) -- (1,1);".into(),
        };
        let json = serde_json::to_string(&raw).unwrap();
        let back: RawLatexBlock = serde_json::from_str(&json).unwrap();
        assert_eq!(back.content, r"\tikz \draw (0,0) -- (1,1);");
    }

    #[test]
    fn list_block() {
        let list = ListBlock {
            meta: BlockMeta::new(),
            marker: ListMarker::Bullet,
            items: vec![
                ListItem {
                    term: None,
                    inlines: vec![Inline::text("First")],
                },
                ListItem {
                    term: None,
                    inlines: vec![Inline::text("Second")],
                },
            ],
        };
        let json = serde_json::to_string(&list).unwrap();
        let back: ListBlock = serde_json::from_str(&json).unwrap();
        assert_eq!(back.marker, ListMarker::Bullet);
        assert_eq!(back.items.len(), 2);
    }

    #[test]
    fn block_tag_serialization() {
        let json = serde_json::to_value(&Block::PageBreak(PageBreak { id: "pb1".into() })).unwrap();
        assert_eq!(json["kind"], "page_break");
    }
}
