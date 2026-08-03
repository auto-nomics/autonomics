//! arXiv-specific translation of the shared [`StructuredSearch`] model.
//!
//! The query model itself ([`StructuredSearch`], [`BoolOp`], [`YearRange`])
//! lives in `bib-types` so that every SDK in the workspace shares the same
//! typed search interface. This module provides [`to_arxiv`], the pure
//! function that renders a `StructuredSearch` into the raw arXiv search query
//! string that the arXiv API expects.
//!
//! ## Translation rules
//!
//! - Each populated field produces one clause.
//! - **Between** fields the join is always `AND`.
//! - **Within** a multi-term field the join defaults to `OR`; `keywords`
//!   honours an explicit [`BoolOp`].
//! - Terms containing whitespace are auto-quoted with double quotes.
//! - `keywords` maps to `all:` (searches all fields — title, abstract,
//!   comments).
//! - `title` maps to `ti:`.
//! - `authors` maps to `au:`.
//! - `journal` maps to `jr:` (journal reference).
//! - `publication_types`, `mesh`, `affiliation` are not directly supported
//!   by arXiv and fall back to `all:` search.
//! - `year_range` maps to a `submittedDate` date-range filter.
//!
//! ## Example
//!
//! ```
//! use bib_types::query::{BoolOp, StructuredSearch, YearRange};
//!
//! let sq = StructuredSearch {
//!     keywords: Some(vec!["transformer".into(), "attention".into()]),
//!     keywords_op: Some(BoolOp::And),
//!     title: Some(vec!["neural network".into()]),
//!     year_range: Some(YearRange { from: 2020, to: 2024 }),
//!     ..Default::default()
//! };
//! let term = arxiv::query::to_arxiv(&sq).unwrap();
//! assert_eq!(
//!     term,
//!     r#"(all:transformer AND all:attention) AND ti:"neural network" "#.to_string()
//!         + r#"AND submittedDate:[202001010000+TO+202412312359]"#
//! );
//! ```

// Re-export the shared model types so existing callers using
// `arxiv::query::StructuredSearch` continue to work.
pub use bib_types::query::{BoolOp, StructuredSearch, YearRange};

use crate::error::{ArxivError, Result};

// ---------------------------------------------------------------------------
// Translation
// ---------------------------------------------------------------------------

/// Translate a [`StructuredSearch`] into an arXiv `search_query` string.
///
/// Returns [`ArxivError::Param`] if the query is empty or the year range is
/// inverted.
pub fn to_arxiv(sq: &StructuredSearch) -> Result<String> {
    if let Some(yr) = &sq.year_range {
        if yr.from > yr.to {
            return Err(ArxivError::Param(format!(
                "year_range inverted: from ({}) > to ({})",
                yr.from, yr.to
            )));
        }
    }

    let mut clauses: Vec<String> = Vec::new();

    // Each multi-term field is filtered to drop blank entries, then rendered
    // as a single clause. Empty fields contribute nothing.
    if let Some(t) = filtered(&sq.keywords) {
        clauses.push(group(&t, "all", sq.keywords_op.unwrap_or_default()));
    }
    if let Some(t) = filtered(&sq.title) {
        clauses.push(group(&t, "ti", BoolOp::Or));
    }
    if let Some(t) = filtered(&sq.authors) {
        clauses.push(group(&t, "au", BoolOp::Or));
    }
    if let Some(t) = filtered(&sq.journal) {
        clauses.push(group(&t, "jr", BoolOp::Or));
    }

    // Fields not natively supported by arXiv fall back to `all:`.
    if let Some(t) = filtered(&sq.publication_types) {
        clauses.push(group(&t, "all", BoolOp::Or));
    }
    if let Some(t) = filtered(&sq.mesh) {
        clauses.push(group(&t, "all", BoolOp::Or));
    }
    if let Some(t) = filtered(&sq.affiliation) {
        clauses.push(group(&t, "all", BoolOp::Or));
    }

    if let Some(yr) = &sq.year_range {
        clauses.push(format!(
            "submittedDate:[{:04}{:02}010000+TO+{:04}{:02}312359]",
            yr.from, 1, yr.to, 12
        ));
    }

    if clauses.is_empty() {
        return Err(ArxivError::Param(
            "structured search is empty: populate at least one field".into(),
        ));
    }

    Ok(clauses.join(" AND "))
}

/// Build a single `(prefix:term1 OP prefix:term2 ...)` clause.
///
/// A lone term is emitted without parentheses. Callers should pre-filter via
/// [`filtered`] so empty terms never reach here.
fn group(terms: &[String], prefix: &str, op: BoolOp) -> String {
    let tagged: Vec<String> = terms
        .iter()
        .map(|t| format!("{prefix}:{}", quote_term(t)))
        .collect();
    let joined = tagged.join(&format!(" {} ", op.keyword()));
    if tagged.len() > 1 {
        format!("({joined})")
    } else {
        joined
    }
}

/// Return a trimmed copy of the field's terms, dropping any that are empty.
/// Returns `None` if the field is unset or contains no non-blank entries.
fn filtered(opt: &Option<Vec<String>>) -> Option<Vec<String>> {
    let v = opt.as_ref()?;
    let cleaned: Vec<String> = v
        .iter()
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
        .collect();
    if cleaned.is_empty() {
        None
    } else {
        Some(cleaned)
    }
}

// ---------------------------------------------------------------------------
// Term quoting
// ---------------------------------------------------------------------------

/// Wrap a term in double quotes iff it contains whitespace.
/// Internal `"` characters are backslash-escaped.
fn quote_term(s: &str) -> String {
    let trimmed = s.trim();
    let needs_quotes = trimmed.chars().any(|c| c.is_whitespace() || c == '"');
    if needs_quotes {
        let escaped = trimmed.replace('"', "\\\"");
        format!("\"{escaped}\"")
    } else {
        trimmed.to_string()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn kw(terms: &[&str]) -> Option<Vec<String>> {
        Some(terms.iter().map(|s| (*s).into()).collect())
    }

    // ----- translation: single field -----------------------------------

    #[test]
    fn single_keyword_no_quotes() {
        let sq = StructuredSearch {
            keywords: kw(&["transformer"]),
            ..Default::default()
        };
        assert_eq!(to_arxiv(&sq).unwrap(), "all:transformer");
    }

    #[test]
    fn single_keyword_with_space_is_quoted() {
        let sq = StructuredSearch {
            keywords: kw(&["neural network"]),
            ..Default::default()
        };
        assert_eq!(to_arxiv(&sq).unwrap(), r#"all:"neural network""#);
    }

    #[test]
    fn keywords_default_or() {
        let sq = StructuredSearch {
            keywords: kw(&["cancer", "tumor"]),
            ..Default::default()
        };
        assert_eq!(to_arxiv(&sq).unwrap(), r#"(all:cancer OR all:tumor)"#);
    }

    #[test]
    fn keywords_explicit_and() {
        let sq = StructuredSearch {
            keywords: kw(&["CRISPR", "review"]),
            keywords_op: Some(BoolOp::And),
            ..Default::default()
        };
        assert_eq!(to_arxiv(&sq).unwrap(), r#"(all:CRISPR AND all:review)"#);
    }

    #[test]
    fn keywords_not_op() {
        let sq = StructuredSearch {
            keywords: kw(&["cancer"]),
            keywords_op: Some(BoolOp::Not),
            ..Default::default()
        };
        // NOT has no visible effect with a single term; we still emit the term.
        assert_eq!(to_arxiv(&sq).unwrap(), "all:cancer");
    }

    // ----- translation: multi-field join --------------------------------

    #[test]
    fn multiple_fields_joined_with_and() {
        let sq = StructuredSearch {
            keywords: kw(&["CRISPR"]),
            authors: kw(&["Smith"]),
            ..Default::default()
        };
        assert_eq!(to_arxiv(&sq).unwrap(), r#"all:CRISPR AND au:Smith"#);
    }

    #[test]
    fn title_field_uses_ti_prefix() {
        let sq = StructuredSearch {
            title: kw(&["BRCA1", "BRCA2"]),
            ..Default::default()
        };
        assert_eq!(to_arxiv(&sq).unwrap(), r#"(ti:BRCA1 OR ti:BRCA2)"#);
    }

    #[test]
    fn author_field_uses_au_prefix() {
        let sq = StructuredSearch {
            authors: kw(&["Hinton", "LeCun"]),
            ..Default::default()
        };
        assert_eq!(to_arxiv(&sq).unwrap(), r#"(au:Hinton OR au:LeCun)"#);
    }

    #[test]
    fn journal_field_uses_jr_prefix() {
        let sq = StructuredSearch {
            journal: kw(&["Nature"]),
            ..Default::default()
        };
        assert_eq!(to_arxiv(&sq).unwrap(), "jr:Nature");
    }

    #[test]
    fn unsupported_fields_fall_back_to_all() {
        let sq = StructuredSearch {
            mesh: kw(&["Genetics"]),
            ..Default::default()
        };
        assert_eq!(to_arxiv(&sq).unwrap(), "all:Genetics");
    }

    // ----- year range ---------------------------------------------------

    #[test]
    fn year_range_emits_date_filter() {
        let sq = StructuredSearch {
            keywords: kw(&["cancer"]),
            year_range: Some(YearRange {
                from: 2020,
                to: 2024,
            }),
            ..Default::default()
        };
        let result = to_arxiv(&sq).unwrap();
        assert!(result.contains("submittedDate:[202001010000+TO+202412312359]"));
    }

    #[test]
    fn year_range_single_year() {
        let sq = StructuredSearch {
            year_range: Some(YearRange {
                from: 2024,
                to: 2024,
            }),
            ..Default::default()
        };
        assert!(
            to_arxiv(&sq)
                .unwrap()
                .contains("submittedDate:[202401010000+TO+202412312359]")
        );
    }

    #[test]
    fn year_range_inverted_errors() {
        let sq = StructuredSearch {
            year_range: Some(YearRange {
                from: 2024,
                to: 2020,
            }),
            ..Default::default()
        };
        let err = to_arxiv(&sq).unwrap_err();
        assert!(matches!(err, ArxivError::Param(_)));
        assert!(format!("{err}").contains("inverted"));
    }

    // ----- errors & edge cases -----------------------------------------

    #[test]
    fn empty_structured_errors() {
        let sq = StructuredSearch::default();
        let err = to_arxiv(&sq).unwrap_err();
        assert!(matches!(err, ArxivError::Param(_)));
    }

    #[test]
    fn all_empty_vecs_treated_as_absent() {
        let sq = StructuredSearch {
            keywords: Some(vec!["".into(), "   ".into()]),
            ..Default::default()
        };
        let err = to_arxiv(&sq).unwrap_err();
        assert!(matches!(err, ArxivError::Param(_)));
    }

    // ----- quoting ------------------------------------------------------

    #[test]
    fn quote_term_bare() {
        assert_eq!(quote_term("CRISPR"), "CRISPR");
        assert_eq!(quote_term("  trim  "), "trim");
    }

    #[test]
    fn quote_term_with_space() {
        assert_eq!(quote_term("lung cancer"), r#""lung cancer""#);
    }

    #[test]
    fn quote_term_escapes_internal_quotes() {
        assert_eq!(quote_term(r#"a"b"#), r#""a\"b""#);
    }
}
