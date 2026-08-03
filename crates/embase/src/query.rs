//! Embase-specific translation of the shared [`StructuredSearch`] model.
//!
//! The query model itself ([`StructuredSearch`], [`BoolOp`], [`YearRange`])
//! lives in `bib-types` so that every SDK in the workspace shares the same
//! typed search interface. This module provides [`to_embase`], the pure
//! function that renders a `StructuredSearch` into the Embase CommandLanguage
//! query string that the search API expects.
//!
//! ## Embase CommandLanguage
//!
//! Embase uses its own search syntax with field qualifiers appended after a
//! colon:
//!
//! | Field           | Tag       | Example                |
//! |-----------------|-----------|------------------------|
//! | Title           | `:ti`     | `'CRISPR':ti`          |
//! | Abstract        | `:ab`     | `'gene editing':ab`    |
//! | Title+Abstract  | `:ti,ab`  | `'cancer':ti,ab`       |
//! | Author          | `:au`     | `'smith j':au`         |
//! | Journal/source  | `:ta`     | `'nature':ta`          |
//! | Publication type| `:dt`     | `'review':dt`          |
//! | Emtree/MeSH     | `:de`     | `'neoplasms':de`       |
//! | Affiliation     | `:af`     | `'harvard':af`         |
//! | Publication year| `:py`     | `2020:2024:py`         |
//!
//! Boolean operators: `AND`, `OR`, `NOT` (uppercase).
//! Phrases containing spaces must be single-quoted.
//!
//! ## Translation rules
//!
//! - Each populated field produces one parenthesised clause.
//! - **Between** fields the join is always `AND`.
//! - **Within** a multi-term field the join defaults to `OR`; `keywords`
//!   honours an explicit [`BoolOp`].
//! - Terms containing whitespace are auto single-quoted.
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
//! let term = embase::query::to_embase(&sq).unwrap();
//! assert_eq!(
//!     term,
//!     "(CRISPR:ti,ab OR 'gene editing':ti,ab) AND Review:dt AND 2020:2024:py"
//! );
//! ```

// Re-export the shared model types so callers using
// `embase::query::StructuredSearch` continue to work.
pub use bib_types::query::{BoolOp, StructuredSearch, YearRange};

use crate::error::{EmbaseError, Result};

// ---------------------------------------------------------------------------
// Translation
// ---------------------------------------------------------------------------

/// Translate a [`StructuredSearch`] into an Embase CommandLanguage query string
/// suitable for the search API's `query` parameter.
///
/// Returns [`EmbaseError::Param`] if the query is empty or the year range is
/// inverted.
pub fn to_embase(sq: &StructuredSearch) -> Result<String> {
    if let Some(yr) = &sq.year_range {
        if yr.from > yr.to {
            return Err(EmbaseError::Param(format!(
                "year_range inverted: from ({}) > to ({})",
                yr.from, yr.to
            )));
        }
    }

    let mut clauses: Vec<String> = Vec::new();

    // Each multi-term field is filtered to drop blank entries, then rendered
    // as a single parenthesised clause. Empty fields contribute nothing.
    if let Some(t) = filtered(&sq.keywords) {
        clauses.push(group(&t, "ti,ab", sq.keywords_op.unwrap_or_default()));
    }
    if let Some(t) = filtered(&sq.title)             { clauses.push(group(&t, "ti",  BoolOp::Or)); }
    if let Some(t) = filtered(&sq.authors)           { clauses.push(group(&t, "au",  BoolOp::Or)); }
    if let Some(t) = filtered(&sq.mesh)              { clauses.push(group(&t, "de",  BoolOp::Or)); }
    if let Some(t) = filtered(&sq.journal)           { clauses.push(group(&t, "ta",  BoolOp::Or)); }
    if let Some(t) = filtered(&sq.publication_types) { clauses.push(group(&t, "dt",  BoolOp::Or)); }
    if let Some(t) = filtered(&sq.affiliation)       { clauses.push(group(&t, "af",  BoolOp::Or)); }

    if let Some(yr) = &sq.year_range {
        clauses.push(format!("{}:{}:py", yr.from, yr.to));
    }

    if clauses.is_empty() {
        return Err(EmbaseError::Param(
            "structured search is empty: populate at least one field".into(),
        ));
    }

    Ok(clauses.join(" AND "))
}

/// Build a single `(t1:field OP t2:field ...)` clause.
///
/// A lone term is emitted without parentheses.
fn group(terms: &[String], field: &str, op: BoolOp) -> String {
    let tagged: Vec<String> = terms
        .iter()
        .map(|t| format!("{}:{}", quote_term(t), field))
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

/// Wrap a term in single quotes iff it contains whitespace or special
/// Embase metacharacters. Internal `'` characters are doubled (Embase
/// convention for escaping within single-quoted strings).
fn quote_term(s: &str) -> String {
    let trimmed = s.trim();
    let needs_quotes = trimmed
        .chars()
        .any(|c| c.is_whitespace() || matches!(c, '\'' | ':' | '(' | ')'));
    if needs_quotes {
        let escaped = trimmed.replace('\'', "''");
        format!("'{escaped}'")
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

    // ----- single field -----

    #[test]
    fn single_keyword_no_quotes() {
        let sq = StructuredSearch {
            keywords: kw(&["CRISPR"]),
            ..Default::default()
        };
        assert_eq!(to_embase(&sq).unwrap(), "CRISPR:ti,ab");
    }

    #[test]
    fn single_keyword_with_space_is_quoted() {
        let sq = StructuredSearch {
            keywords: kw(&["lung cancer"]),
            ..Default::default()
        };
        assert_eq!(to_embase(&sq).unwrap(), "'lung cancer':ti,ab");
    }

    #[test]
    fn keywords_default_or() {
        let sq = StructuredSearch {
            keywords: kw(&["cancer", "neoplasm"]),
            ..Default::default()
        };
        assert_eq!(
            to_embase(&sq).unwrap(),
            "(cancer:ti,ab OR neoplasm:ti,ab)"
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
            to_embase(&sq).unwrap(),
            "(CRISPR:ti,ab AND review:ti,ab)"
        );
    }

    // ----- multi-field -----

    #[test]
    fn multiple_fields_joined_with_and() {
        let sq = StructuredSearch {
            keywords: kw(&["CRISPR"]),
            authors: kw(&["Smith J"]),
            publication_types: kw(&["Review"]),
            ..Default::default()
        };
        assert_eq!(
            to_embase(&sq).unwrap(),
            "CRISPR:ti,ab AND 'Smith J':au AND Review:dt"
        );
    }

    #[test]
    fn title_field_uses_ti_tag() {
        let sq = StructuredSearch {
            title: kw(&["BRCA1", "BRCA2"]),
            ..Default::default()
        };
        assert_eq!(
            to_embase(&sq).unwrap(),
            "(BRCA1:ti OR BRCA2:ti)"
        );
    }

    #[test]
    fn mesh_field_uses_de_tag() {
        let sq = StructuredSearch {
            mesh: kw(&["Neoplasms"]),
            ..Default::default()
        };
        assert_eq!(to_embase(&sq).unwrap(), "Neoplasms:de");
    }

    #[test]
    fn journal_field_uses_ta_tag() {
        let sq = StructuredSearch {
            journal: kw(&["Nature", "Science"]),
            ..Default::default()
        };
        assert_eq!(
            to_embase(&sq).unwrap(),
            "(Nature:ta OR Science:ta)"
        );
    }

    #[test]
    fn affiliation_field_quotes_spaces() {
        let sq = StructuredSearch {
            affiliation: kw(&["Harvard Medical School"]),
            ..Default::default()
        };
        assert_eq!(
            to_embase(&sq).unwrap(),
            "'Harvard Medical School':af"
        );
    }

    // ----- year range -----

    #[test]
    fn year_range_emits_embase_range() {
        let sq = StructuredSearch {
            keywords: kw(&["cancer"]),
            year_range: Some(YearRange { from: 2020, to: 2024 }),
            ..Default::default()
        };
        assert_eq!(
            to_embase(&sq).unwrap(),
            "cancer:ti,ab AND 2020:2024:py"
        );
    }

    #[test]
    fn year_range_single_year() {
        let sq = StructuredSearch {
            year_range: Some(YearRange { from: 2024, to: 2024 }),
            ..Default::default()
        };
        assert_eq!(to_embase(&sq).unwrap(), "2024:2024:py");
    }

    #[test]
    fn year_range_inverted_errors() {
        let sq = StructuredSearch {
            year_range: Some(YearRange { from: 2024, to: 2020 }),
            ..Default::default()
        };
        let err = to_embase(&sq).unwrap_err();
        assert!(matches!(err, EmbaseError::Param(_)));
        assert!(format!("{err}").contains("inverted"));
    }

    // ----- errors & edge cases -----

    #[test]
    fn empty_structured_errors() {
        let sq = StructuredSearch::default();
        let err = to_embase(&sq).unwrap_err();
        assert!(matches!(err, EmbaseError::Param(_)));
    }

    #[test]
    fn all_empty_vecs_treated_as_absent() {
        let sq = StructuredSearch {
            keywords: Some(vec!["".into(), "   ".into()]),
            ..Default::default()
        };
        let err = to_embase(&sq).unwrap_err();
        assert!(matches!(err, EmbaseError::Param(_)));
    }

    // ----- quoting -----

    #[test]
    fn quote_term_bare() {
        assert_eq!(quote_term("CRISPR"), "CRISPR");
        assert_eq!(quote_term("  trim  "), "trim");
    }

    #[test]
    fn quote_term_with_space() {
        assert_eq!(quote_term("lung cancer"), "'lung cancer'");
    }

    #[test]
    fn quote_term_with_colon() {
        assert_eq!(quote_term("a:b"), "'a:b'");
    }

    #[test]
    fn quote_term_escapes_internal_quotes() {
        assert_eq!(quote_term("o'brien"), "'o''brien'");
    }
}
