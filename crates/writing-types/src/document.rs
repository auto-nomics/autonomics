//! Top-level document model — a single writing project (paper, report, thesis).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::section::Section;

// ---------------------------------------------------------------------------
// Document
// ---------------------------------------------------------------------------

/// A writing project — one paper, report, or book chapter.
///
/// The document owns a [`Section`] tree plus preamble and metadata.  All
/// agent edits operate on this structure; LaTeX is merely a serialisation
/// target produced from it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Document {
    /// Internal unique identifier.
    pub id: String,

    /// Working title (may contain LaTeX-free text).
    pub title: String,

    /// Authors of *this* document (not to be confused with bibliography
    /// authors in `bib-types`).
    #[serde(default)]
    pub authors: Vec<DocumentAuthor>,

    /// LaTeX document class.
    #[serde(default)]
    pub document_class: DocumentClass,

    /// Optional template binding (journal / conference preset).
    #[serde(default)]
    pub template_id: Option<String>,

    /// Section tree root.
    #[serde(default)]
    pub root: Section,

    /// Custom preamble (packages, macros, arbitrary lines).
    #[serde(default)]
    pub preamble: Preamble,

    /// Structured metadata (keywords, abstract, subject codes).
    #[serde(default)]
    pub metadata: DocumentMetadata,

    /// Optimistic-locking version number — incremented on every save.
    #[serde(default)]
    pub version: u32,

    #[serde(default)]
    pub created_at: Option<DateTime<Utc>>,

    #[serde(default)]
    pub updated_at: Option<DateTime<Utc>>,
}

impl Document {
    /// Create a new empty document with the given title.
    pub fn new(id: impl Into<String>, title: impl Into<String>) -> Self {
        let now = Utc::now();
        Self {
            id: id.into(),
            title: title.into(),
            authors: Vec::new(),
            document_class: DocumentClass::Article,
            template_id: None,
            root: Section::root(),
            preamble: Preamble::default(),
            metadata: DocumentMetadata::default(),
            version: 0,
            created_at: Some(now),
            updated_at: Some(now),
        }
    }
}

// ---------------------------------------------------------------------------
// Document author
// ---------------------------------------------------------------------------

/// An author of the document under construction.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DocumentAuthor {
    pub name: String,
    #[serde(default)]
    pub affiliation: Option<String>,
    #[serde(default)]
    pub orcid: Option<String>,
    #[serde(default)]
    pub corresponding: bool,
    #[serde(default)]
    pub email: Option<String>,
}

// ---------------------------------------------------------------------------
// Document class
// ---------------------------------------------------------------------------

/// LaTeX `\documentclass` selector.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DocumentClass {
    Article,
    Report,
    Book,
    Beamer,
    /// Custom class name (e.g. `revtex4-2`, `elsarticle`, `ctexart`).
    Custom(String),
}

impl Default for DocumentClass {
    fn default() -> Self {
        Self::Article
    }
}

impl DocumentClass {
    /// Render the `\documentclass` command (without options).
    pub fn class_name(&self) -> &str {
        match self {
            Self::Article => "article",
            Self::Report => "report",
            Self::Book => "book",
            Self::Beamer => "beamer",
            Self::Custom(s) => s,
        }
    }
}

// ---------------------------------------------------------------------------
// Preamble
// ---------------------------------------------------------------------------

/// Custom preamble content.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Preamble {
    /// `\usepackage{...}` entries (without the wrapping command).
    #[serde(default)]
    pub packages: Vec<String>,

    /// Custom macro definitions.
    #[serde(default)]
    pub macros: Vec<MacroDef>,

    /// Arbitrary preamble lines — escape hatch for anything not covered by
    /// `packages` or `macros`.
    #[serde(default)]
    pub custom: Vec<String>,
}

/// A `\newcommand` definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MacroDef {
    pub name: String,
    pub n_args: u8,
    pub body: String,
}

// ---------------------------------------------------------------------------
// Document metadata
// ---------------------------------------------------------------------------

/// Structured metadata that lives outside the body text.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DocumentMetadata {
    /// Free-text abstract (plain, no LaTeX).
    #[serde(default)]
    pub abstract_text: Option<String>,

    /// Author-specified keywords.
    #[serde(default)]
    pub keywords: Vec<String>,

    /// Subject classification codes (JEL, MSC, AMS, …).
    #[serde(default)]
    pub subject_codes: Vec<String>,

    /// Acknowledgements text.
    #[serde(default)]
    pub acknowledgements: Option<String>,

    /// Funding statement.
    #[serde(default)]
    pub funding: Option<String>,

    /// Conflict-of-interest statement.
    #[serde(default)]
    pub conflicts: Option<String>,

    /// Data-availability statement.
    #[serde(default)]
    pub data_availability: Option<String>,
}

/// Lightweight template specification (full templates live in `writing-base`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TemplateSpec {
    pub id: String,
    pub name: String,
    pub document_class: String,
    #[serde(default)]
    pub class_options: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn document_new_defaults() {
        let doc = Document::new("d1", "My Paper");
        assert_eq!(doc.title, "My Paper");
        assert_eq!(doc.document_class, DocumentClass::Article);
        assert_eq!(doc.version, 0);
        assert!(doc.authors.is_empty());
        assert!(doc.root.blocks.is_empty());
        assert!(doc.root.children.is_empty());
    }

    #[test]
    fn document_class_name() {
        assert_eq!(DocumentClass::Article.class_name(), "article");
        assert_eq!(DocumentClass::Beamer.class_name(), "beamer");
        assert_eq!(
            DocumentClass::Custom("ctexart".into()).class_name(),
            "ctexart"
        );
    }

    #[test]
    fn json_round_trip() {
        let mut doc = Document::new("d1", "Test");
        doc.authors.push(DocumentAuthor {
            name: "Jane Doe".into(),
            affiliation: Some("MIT".into()),
            orcid: Some("0000-0001-0002-0003".into()),
            corresponding: true,
            email: Some("jane@mit.edu".into()),
        });
        doc.metadata.keywords = vec!["GWAS".into(), "LDSC".into()];
        doc.metadata.abstract_text = Some("A test paper.".into());

        let json = serde_json::to_string_pretty(&doc).unwrap();
        let back: Document = serde_json::from_str(&json).unwrap();

        assert_eq!(back.id, doc.id);
        assert_eq!(back.title, doc.title);
        assert_eq!(back.authors.len(), 1);
        assert_eq!(back.authors[0].name, "Jane Doe");
        assert_eq!(back.metadata.keywords, doc.metadata.keywords);
    }

    #[test]
    fn preamble_serialization() {
        let mut preamble = Preamble::default();
        preamble.packages.push("amsmath".into());
        preamble.macros.push(MacroDef {
            name: "real".into(),
            n_args: 0,
            body: r"\mathbb{R}".into(),
        });
        let json = serde_json::to_string(&preamble).unwrap();
        let back: Preamble = serde_json::from_str(&json).unwrap();
        assert_eq!(back.packages, vec!["amsmath"]);
        assert_eq!(back.macros.len(), 1);
    }
}
