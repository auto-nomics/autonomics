//! Structured literature-search query model.
//!
//! [`StructuredSearch`] is a backend-agnostic, typed description of a
//! bibliographic search. It is shared across all SDKs in the workspace
//! (`eutils`, `gwascatalog-sdk`, …). Each SDK provides its own translation
//! function to render a `StructuredSearch` into the native query syntax of
//! its target API.
//!
//! ## Field semantics
//!
//! - Each populated field represents one facet of the search.
//! - **Between** fields the logical join is `AND`.
//! - **Within** a multi-term field the join defaults to `OR`; the `keywords`
//!   field additionally honours an explicit [`BoolOp`] (e.g. set
//!   `keywords_op = And` for a narrow topic intersection).
//! - Authors / journal / MeSH / etc. always use `OR` internally — that matches
//!   the common "any of these" intent.
//!
//! ## Example
//!
//! ```
//! use bib_types::query::{BoolOp, StructuredSearch, YearRange};
//!
//! let sq = StructuredSearch {
//!     keywords: Some(vec!["CRISPR".into(), "gene editing".into()]),
//!     keywords_op: Some(BoolOp::Or),
//!     publication_types: Some(vec!["Review".into()]),
//!     year_range: Some(YearRange { from: 2020, to: 2024 }),
//!     ..Default::default()
//! };
//! ```

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Model
// ---------------------------------------------------------------------------

/// Boolean operator used to join terms *within* a single field of a
/// [`StructuredSearch`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "UPPERCASE")]
pub enum BoolOp {
    And,
    #[default]
    Or,
    Not,
}

impl BoolOp {
    /// Return the keyword representation (`"AND"`, `"OR"`, `"NOT"`).
    pub fn keyword(self) -> &'static str {
        match self {
            BoolOp::And => "AND",
            BoolOp::Or => "OR",
            BoolOp::Not => "NOT",
        }
    }
}

/// Inclusive publication-year range.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct YearRange {
    /// Lower bound (e.g. `2020`).
    pub from: u16,
    /// Upper bound (e.g. `2024`).
    pub to: u16,
}

/// Structured description of a literature search.
///
/// All fields are optional; populate only the ones you need. At least one
/// field should be set for a meaningful query — individual backends decide
/// how to report an empty query.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct StructuredSearch {
    /// Topic terms searched against title/abstract — the most common slot.
    pub keywords: Option<Vec<String>>,

    /// Operator joining `keywords` internally. Defaults to `OR`; set to `AND`
    /// to require all keywords, or `NOT` to exclude them.
    pub keywords_op: Option<BoolOp>,

    /// Terms restricted to the title field only.
    pub title: Option<Vec<String>>,

    /// Author names, e.g. `["Smith J", "Doe K"]`. Joined with `OR`.
    pub authors: Option<Vec<String>>,

    /// MeSH / subject headings, e.g. `["Neoplasms"]`. `OR`.
    pub mesh: Option<Vec<String>>,

    /// Journal names (full or ISO abbreviation). `OR`.
    pub journal: Option<Vec<String>>,

    /// Publication types, e.g. `["Review", "Clinical Trial"]`. `OR`.
    pub publication_types: Option<Vec<String>>,

    /// Affiliation keywords (institutions, departments). `OR`.
    pub affiliation: Option<Vec<String>>,

    /// Inclusive publication-year window.
    pub year_range: Option<YearRange>,
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn boolop_default_is_or() {
        assert_eq!(BoolOp::default(), BoolOp::Or);
    }

    #[test]
    fn boolop_keywords() {
        assert_eq!(BoolOp::And.keyword(), "AND");
        assert_eq!(BoolOp::Or.keyword(), "OR");
        assert_eq!(BoolOp::Not.keyword(), "NOT");
    }

    #[test]
    fn boolop_serde_uppercase() {
        assert_eq!(serde_json::to_string(&BoolOp::And).unwrap(), r#""AND""#);
        assert_eq!(serde_json::to_string(&BoolOp::Or).unwrap(), r#""OR""#);
        assert_eq!(serde_json::to_string(&BoolOp::Not).unwrap(), r#""NOT""#);
        let op: BoolOp = serde_json::from_str("\"AND\"").unwrap();
        assert_eq!(op, BoolOp::And);
    }

    #[test]
    fn structured_search_default_all_none() {
        let sq = StructuredSearch::default();
        assert!(sq.keywords.is_none());
        assert!(sq.year_range.is_none());
    }

    #[test]
    fn serde_roundtrip() {
        let sq = StructuredSearch {
            keywords: Some(vec!["a".into(), "b".into()]),
            keywords_op: Some(BoolOp::And),
            year_range: Some(YearRange {
                from: 2018,
                to: 2023,
            }),
            ..Default::default()
        };
        let json = serde_json::to_string(&sq).unwrap();
        let back: StructuredSearch = serde_json::from_str(&json).unwrap();
        assert_eq!(sq.keywords, back.keywords);
        assert_eq!(sq.keywords_op, back.keywords_op);
        assert_eq!(sq.year_range, back.year_range);
    }
}
