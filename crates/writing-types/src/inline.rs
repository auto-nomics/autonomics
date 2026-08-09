//! Inline content — text, citations, math, cross-references within a paragraph.

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// BlockId alias (re-exported from block.rs but defined here to avoid a
// circular module dependency — block.rs imports from this module).
// ---------------------------------------------------------------------------

/// Stable identifier for any addressable node in the document tree.
pub type BlockId = String;

// ---------------------------------------------------------------------------
// Inline
// ---------------------------------------------------------------------------

/// A piece of inline content within a [`ParagraphBlock`](crate::ParagraphBlock).
///
/// Variants are intentionally flat so that a paragraph is a simple
/// `Vec<Inline>` — no need for recursive nesting except for footnotes.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Inline {
    /// Plain text.
    Text { content: String },

    /// Formatted text run.
    Formatted {
        content: String,
        #[serde(default)]
        format: TextFormat,
    },

    /// Inline math `$...$`.
    InlineMath { latex: String },

    /// A citation cluster (one or more cite keys at a single location).
    Citation(CitationCluster),

    /// Cross-reference to a label.
    CrossRef {
        label: String,
        #[serde(default)]
        kind: RefKind,
        /// Use `\cref` (auto-prefix) instead of `\ref`.
        #[serde(default)]
        auto_prefix: bool,
    },

    /// Hyperlink.
    Link {
        url: String,
        #[serde(default)]
        text: String,
    },

    /// Footnote.
    Footnote {
        #[serde(default)]
        content: Vec<Inline>,
    },
}

impl Inline {
    /// Convenience constructor for plain text.
    pub fn text(content: impl Into<String>) -> Self {
        Self::Text {
            content: content.into(),
        }
    }

    /// Convenience constructor for formatted text.
    pub fn formatted(content: impl Into<String>, format: TextFormat) -> Self {
        Self::Formatted {
            content: content.into(),
            format,
        }
    }

    /// Convenience constructor for inline math.
    pub fn math(latex: impl Into<String>) -> Self {
        Self::InlineMath {
            latex: latex.into(),
        }
    }

    /// Extract plain-text representation (for word counting, search, etc.).
    pub fn plain_text(&self) -> String {
        match self {
            Self::Text { content } => content.clone(),
            Self::Formatted { content, .. } => content.clone(),
            Self::InlineMath { .. } => String::new(),
            Self::Citation(c) => {
                let keys: Vec<&str> = c.keys.iter().map(|k| k.key.as_str()).collect();
                format!("[{}]", keys.join("; "))
            }
            Self::CrossRef { label, .. } => format!("[ref:{label}]"),
            Self::Link { text, .. } => text.clone(),
            Self::Footnote { content } => {
                content.iter().map(|i| i.plain_text()).collect()
            }
        }
    }
}

// ---------------------------------------------------------------------------
// TextFormat
// ---------------------------------------------------------------------------

/// Text formatting applied to a run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TextFormat {
    #[default]
    Bold,
    Italic,
    Underline,
    Strikethrough,
    Monospace,
    SmallCaps,
}

// ---------------------------------------------------------------------------
// Citation
// ---------------------------------------------------------------------------

/// A citation cluster — one or more cite keys at a single position.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CitationCluster {
    /// Cite keys in order.
    pub keys: Vec<CiteKey>,
    /// Citation command style.
    #[serde(default)]
    pub style: CitationStyle,
    /// Optional prefix text (e.g. "see").
    #[serde(default)]
    pub prefix: Option<String>,
    /// Optional suffix text (e.g. "p. 15").
    #[serde(default)]
    pub suffix: Option<String>,
    /// Associated claim ID if this citation supports a claim.
    #[serde(default)]
    pub claim_id: Option<String>,
}

/// A single cite key pointing to a `bib-base` Article.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CiteKey {
    /// The BibTeX key (e.g. `smith2024breakthrough`).
    pub key: String,
    /// Optional `bib-base` article ID (filled by `CitationResolver`).
    #[serde(default)]
    pub article_id: Option<String>,
}

impl CiteKey {
    pub fn new(key: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            article_id: None,
        }
    }
}

/// Citation command style — maps to natbib / biblatex commands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CitationStyle {
    /// `\cite{key}`
    Plain,
    /// `\citep{key}` — (Author, Year)
    Parenthetical,
    /// `\citet{key}` — Author (Year)
    Textual,
    /// `\citeyear{key}` — Year
    YearOnly,
    /// `\citeauthor{key}` — Author
    AuthorOnly,
    /// `\footcite{key}`
    Footnote,
}

impl Default for CitationStyle {
    fn default() -> Self {
        Self::Parenthetical
    }
}

impl CitationStyle {
    /// The natbib command for this style.
    pub fn natbib_cmd(&self) -> &'static str {
        match self {
            Self::Plain => "cite",
            Self::Parenthetical => "citep",
            Self::Textual => "citet",
            Self::YearOnly => "citeyear",
            Self::AuthorOnly => "citeauthor",
            Self::Footnote => "footcite",
        }
    }

    /// The biblatex command for this style.
    pub fn biblatex_cmd(&self) -> &'static str {
        match self {
            Self::Plain => "cite",
            Self::Parenthetical => "parencite",
            Self::Textual => "textcite",
            Self::YearOnly => "citeyear",
            Self::AuthorOnly => "citeauthor",
            Self::Footnote => "footcite",
        }
    }
}

// ---------------------------------------------------------------------------
// RefKind
// ---------------------------------------------------------------------------

/// Type of cross-reference target.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RefKind {
    /// Equation.
    Eq,
    /// Figure.
    Fig,
    /// Table.
    Tab,
    /// Section.
    Sec,
    /// Auto-detect (requires `cleveref`).
    Auto,
}

impl Default for RefKind {
    fn default() -> Self {
        Self::Auto
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inline_plain_text() {
        assert_eq!(Inline::text("hello").plain_text(), "hello");
        assert_eq!(
            Inline::formatted("bold", TextFormat::Bold).plain_text(),
            "bold"
        );
        assert_eq!(Inline::math("x^2").plain_text(), "");
    }

    #[test]
    fn citation_plain_text() {
        let c = Inline::Citation(CitationCluster {
            keys: vec![CiteKey::new("smith2024"), CiteKey::new("jones2023")],
            style: CitationStyle::Parenthetical,
            prefix: None,
            suffix: None,
            claim_id: None,
        });
        assert_eq!(c.plain_text(), "[smith2024; jones2023]");
    }

    #[test]
    fn footnote_plain_text_recursive() {
        let f = Inline::Footnote {
            content: vec![Inline::text("note text")],
        };
        assert_eq!(f.plain_text(), "note text");
    }

    #[test]
    fn citation_style_commands() {
        assert_eq!(CitationStyle::Parenthetical.natbib_cmd(), "citep");
        assert_eq!(CitationStyle::Textual.natbib_cmd(), "citet");
        assert_eq!(CitationStyle::Parenthetical.biblatex_cmd(), "parencite");
        assert_eq!(CitationStyle::Textual.biblatex_cmd(), "textcite");
    }

    #[test]
    fn citation_cluster_round_trip() {
        let cluster = CitationCluster {
            keys: vec![CiteKey {
                key: "doe2021".into(),
                article_id: Some("art-123".into()),
            }],
            style: CitationStyle::Textual,
            prefix: Some("see".into()),
            suffix: Some("p. 5".into()),
            claim_id: None,
        };
        let json = serde_json::to_string(&cluster).unwrap();
        let back: CitationCluster = serde_json::from_str(&json).unwrap();
        assert_eq!(back.keys.len(), 1);
        assert_eq!(back.style, CitationStyle::Textual);
        assert_eq!(back.prefix.as_deref(), Some("see"));
    }

    #[test]
    fn inline_serialization_tag() {
        let t = Inline::text("hello");
        let json = serde_json::to_value(&t).unwrap();
        assert_eq!(json["type"], "text");
        assert_eq!(json["content"], "hello");

        let m = Inline::math("x^2");
        let json = serde_json::to_value(&m).unwrap();
        assert_eq!(json["type"], "inline_math");
    }
}
