//! Entrez-specific translation of the shared [`StructuredSearch`] model.
//!
//! The query model itself ([`StructuredSearch`], [`BoolOp`], [`YearRange`])
//! lives in `bib-types` so that every SDK in the workspace shares the same
//! typed search interface. This module provides [`to_entrez`], the pure
//! function that renders a `StructuredSearch` into the raw Entrez query
//! string that NCBI ESearch expects.
//!
//! ## Translation rules
//!
//! - Each populated field produces one parenthesised clause.
//! - **Between** fields the join is always `AND`.
//! - **Within** a multi-term field the join defaults to `OR`; `keywords`
//!   honours an explicit [`BoolOp`].
//! - Terms containing whitespace or Entrez metacharacters are auto-quoted.
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
//! let term = eutils::query::to_entrez(&sq).unwrap();
//! assert_eq!(
//!     term,
//!     r#"(CRISPR[Title/Abstract] OR "gene editing"[Title/Abstract]) "#.to_string()
//!         + r#"AND Review[Publication Type] AND 2020:2024[Year]"#
//! );
//! ```

// Re-export the shared model types so existing callers using
// `eutils::query::StructuredSearch` continue to work.
pub use bib_types::query::{BoolOp, StructuredSearch, YearRange};

use crate::error::{EutilsError, Result};

// ---------------------------------------------------------------------------
// Translation
// ---------------------------------------------------------------------------

/// Translate a [`StructuredSearch`] into an Entrez query string suitable for
/// `ESearch`'s `term` parameter.
///
/// Returns [`EutilsError::Param`] if the query is empty or the year range is
/// inverted.
pub fn to_entrez(sq: &StructuredSearch) -> Result<String> {
    // Validate year range up front so users get a clear error instead of an
    // NCBI-side silent zero-result query.
    if let Some(yr) = &sq.year_range {
        if yr.from > yr.to {
            return Err(EutilsError::Param(format!(
                "year_range inverted: from ({}) > to ({})",
                yr.from, yr.to
            )));
        }
    }

    let mut clauses: Vec<String> = Vec::new();

    // Each multi-term field is filtered to drop blank entries, then rendered
    // as a single parenthesised clause. Empty fields contribute nothing.
    if let Some(t) = filtered(&sq.keywords) {
        clauses.push(group(
            &t,
            "Title/Abstract",
            sq.keywords_op.unwrap_or_default(),
        ));
    }
    if let Some(t) = filtered(&sq.title) {
        clauses.push(group(&t, "Title", BoolOp::Or));
    }
    if let Some(t) = filtered(&sq.authors) {
        clauses.push(group(&t, "Author", BoolOp::Or));
    }
    if let Some(t) = filtered(&sq.mesh) {
        clauses.push(group(&t, "MeSH", BoolOp::Or));
    }
    if let Some(t) = filtered(&sq.journal) {
        clauses.push(group(&t, "Journal", BoolOp::Or));
    }
    if let Some(t) = filtered(&sq.publication_types) {
        clauses.push(group(&t, "Publication Type", BoolOp::Or));
    }
    if let Some(t) = filtered(&sq.affiliation) {
        clauses.push(group(&t, "Affiliation", BoolOp::Or));
    }

    if let Some(yr) = &sq.year_range {
        clauses.push(format!("{}:{}[Year]", yr.from, yr.to));
    }

    if clauses.is_empty() {
        return Err(EutilsError::Param(
            "structured search is empty: populate at least one field".into(),
        ));
    }

    Ok(clauses.join(" AND "))
}

/// Build a single `(t1[Field] OP t2[Field] ...)` clause.
///
/// A lone term is emitted without parentheses. Callers should pre-filter via
/// [`filtered`] so empty terms never reach here.
fn group(terms: &[String], field: &str, op: BoolOp) -> String {
    let tagged: Vec<String> = terms
        .iter()
        .map(|t| format!("{}[{}]", quote_term(t), field))
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

/// Wrap a term in double quotes iff it contains whitespace or Entrez
/// metacharacters. Internal `"` characters are backslash-escaped.
fn quote_term(s: &str) -> String {
    let trimmed = s.trim();
    let needs_quotes = trimmed
        .chars()
        .any(|c| c.is_whitespace() || matches!(c, '(' | ')' | '[' | ']' | '"'));
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
            keywords: kw(&["CRISPR"]),
            ..Default::default()
        };
        assert_eq!(to_entrez(&sq).unwrap(), "CRISPR[Title/Abstract]");
    }

    #[test]
    fn single_keyword_with_space_is_quoted() {
        let sq = StructuredSearch {
            keywords: kw(&["lung cancer"]),
            ..Default::default()
        };
        assert_eq!(to_entrez(&sq).unwrap(), r#""lung cancer"[Title/Abstract]"#);
    }

    #[test]
    fn keywords_default_or() {
        let sq = StructuredSearch {
            keywords: kw(&["cancer", "neoplasm"]),
            ..Default::default()
        };
        assert_eq!(
            to_entrez(&sq).unwrap(),
            r#"(cancer[Title/Abstract] OR neoplasm[Title/Abstract])"#
        );
    }

    #[test]
    fn keywords_explicit_and() {
        let sq = StructuredSearch {
            keywords: kw(&["CRISPR", "review"]),
            keywords_op: Some(BoolOp::And),
            ..Default::default()
        };
        assert_eq!(
            to_entrez(&sq).unwrap(),
            r#"(CRISPR[Title/Abstract] AND review[Title/Abstract])"#
        );
    }

    #[test]
    fn keywords_not_op() {
        let sq = StructuredSearch {
            keywords: kw(&["cancer"]),
            keywords_op: Some(BoolOp::Not),
            ..Default::default()
        };
        // NOT has no visible effect with a single term; we still emit the term.
        assert_eq!(to_entrez(&sq).unwrap(), "cancer[Title/Abstract]");
    }

    // ----- translation: multi-field join --------------------------------

    #[test]
    fn multiple_fields_joined_with_and() {
        let sq = StructuredSearch {
            keywords: kw(&["CRISPR"]),
            authors: kw(&["Smith J"]),
            publication_types: kw(&["Review"]),
            ..Default::default()
        };
        assert_eq!(
            to_entrez(&sq).unwrap(),
            r#"CRISPR[Title/Abstract] AND "Smith J"[Author] AND Review[Publication Type]"#
        );
    }

    #[test]
    fn title_field_uses_title_tag() {
        let sq = StructuredSearch {
            title: kw(&["BRCA1", "BRCA2"]),
            ..Default::default()
        };
        assert_eq!(to_entrez(&sq).unwrap(), r#"(BRCA1[Title] OR BRCA2[Title])"#);
    }

    #[test]
    fn mesh_field() {
        let sq = StructuredSearch {
            mesh: kw(&["Neoplasms"]),
            ..Default::default()
        };
        assert_eq!(to_entrez(&sq).unwrap(), "Neoplasms[MeSH]");
    }

    #[test]
    fn journal_field() {
        let sq = StructuredSearch {
            journal: kw(&["Nature", "Science"]),
            ..Default::default()
        };
        assert_eq!(
            to_entrez(&sq).unwrap(),
            r#"(Nature[Journal] OR Science[Journal])"#
        );
    }

    #[test]
    fn affiliation_field_quotes_spaces() {
        let sq = StructuredSearch {
            affiliation: kw(&["Harvard Medical School"]),
            ..Default::default()
        };
        assert_eq!(
            to_entrez(&sq).unwrap(),
            r#""Harvard Medical School"[Affiliation]"#
        );
    }

    // ----- year range ---------------------------------------------------

    #[test]
    fn year_range_emits_entrez_range() {
        let sq = StructuredSearch {
            keywords: kw(&["cancer"]),
            year_range: Some(YearRange {
                from: 2020,
                to: 2024,
            }),
            ..Default::default()
        };
        assert_eq!(
            to_entrez(&sq).unwrap(),
            r#"cancer[Title/Abstract] AND 2020:2024[Year]"#
        );
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
        assert_eq!(to_entrez(&sq).unwrap(), "2024:2024[Year]");
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
        let err = to_entrez(&sq).unwrap_err();
        assert!(matches!(err, EutilsError::Param(_)));
        assert!(format!("{err}").contains("inverted"));
    }

    // ----- errors & edge cases -----------------------------------------

    #[test]
    fn empty_structured_errors() {
        let sq = StructuredSearch::default();
        let err = to_entrez(&sq).unwrap_err();
        assert!(matches!(err, EutilsError::Param(_)));
    }

    #[test]
    fn all_empty_vecs_treated_as_absent() {
        let sq = StructuredSearch {
            keywords: Some(vec!["".into(), "   ".into()]),
            ..Default::default()
        };
        let err = to_entrez(&sq).unwrap_err();
        assert!(matches!(err, EutilsError::Param(_)));
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
    fn quote_term_with_metachars() {
        assert_eq!(quote_term("foo(bar)"), r#""foo(bar)""#);
        assert_eq!(quote_term("a[b]c"), r#""a[b]c""#);
    }

    #[test]
    fn quote_term_escapes_internal_quotes() {
        assert_eq!(quote_term(r#"a"b"#), r#""a\"b""#);
    }
}
