//! Europe PMC-specific translation of the shared [`StructuredSearch`] model.
//!
//! The query model itself ([`StructuredSearch`], [`BoolOp`], [`YearRange`])
//! lives in `bib-types` so that every SDK in the workspace shares the same
//! typed search interface. This module provides [`to_europepmc`], the pure
//! function that renders a `StructuredSearch` into the raw Europe PMC query
//! string that the search API expects.
//!
//! ## Europe PMC search syntax
//!
//! | Field           | Tag          | Example                    |
//! |-----------------|--------------|----------------------------|
//! | Title           | `TITLE:`     | `TITLE:cancer`             |
//! | Abstract        | `ABSTRACT:`  | `ABSTRACT:cancer`          |
//! | Author          | `AUTH:`      | `AUTH:Smith`               |
//! | Journal         | `JOURNAL:`   | `JOURNAL:Nature`           |
//! | Keyword         | `KEYWORD:`   | `KEYWORD:cancer`           |
//! | MeSH heading    | `MESH:`      | `MESH:"Neoplasms"`         |
//! | Affiliation     | `AFF:`       | `AFF:Harvard`              |
//! | Publication type| `PUB_TYPE:`  | `PUB_TYPE:Review`          |
//! | Publication year| `PUB_YEAR:`  | `PUB_YEAR:2024`            |
//! | Year range      | `PUB_YEAR:`  | `(PUB_YEAR:[2020 TO 2024])`|
//!
//! Boolean operators (uppercase): `AND`, `OR`, `NOT`. Group with parentheses.
//! Phrase-quote with double quotes.
//!
//! ## Translation rules
//!
//! - Each populated field produces one clause.
//! - **Between** fields the join is always `AND`.
//! - **Within** a multi-term field the join defaults to `OR`; `keywords`
//!   honours an explicit [`BoolOp`].
//! - Terms containing whitespace are auto-quoted with double quotes.
//!
//! ## Example
//!
//! ```
//! use bib_types::query::{BoolOp, StructuredSearch, YearRange};
//!
//! let sq = StructuredSearch {
//!     keywords: Some(vec!["p53".into(), "cancer".into()]),
//!     keywords_op: Some(BoolOp::And),
//!     authors: Some(vec!["Smith".into()]),
//!     year_range: Some(YearRange { from: 2020, to: 2024 }),
//!     ..Default::default()
//! };
//! let term = europepmc::query::to_europepmc(&sq).unwrap();
//! assert_eq!(
//!     term,
//!     r#"(p53 AND cancer) AND AUTH:Smith AND (PUB_YEAR:[2020 TO 2024])"#
//! );
//! ```

// Re-export the shared model types so callers using
// `europepmc::query::StructuredSearch` continue to work.
pub use bib_types::query::{BoolOp, StructuredSearch, YearRange};

use crate::error::{EuropePmcError, Result};

// ---------------------------------------------------------------------------
// Translation
// ---------------------------------------------------------------------------

/// Translate a [`StructuredSearch`] into a Europe PMC query string suitable
/// for the search API's `query` parameter.
///
/// Returns [`EuropePmcError::Param`] if the query is empty or the year range
/// is inverted.
pub fn to_europepmc(sq: &StructuredSearch) -> Result<String> {
    if let Some(yr) = &sq.year_range {
        if yr.from > yr.to {
            return Err(EuropePmcError::Param(format!(
                "year_range inverted: from ({}) > to ({})",
                yr.from, yr.to
            )));
        }
    }

    let mut clauses: Vec<String> = Vec::new();

    // `keywords` maps to a bare all-field search — Europe PMC searches all
    // indexed fields when no field prefix is given.
    if let Some(t) = filtered(&sq.keywords) {
        clauses.push(group(&t, "", sq.keywords_op.unwrap_or_default()));
    }
    if let Some(t) = filtered(&sq.title) {
        clauses.push(group(&t, "TITLE", BoolOp::Or));
    }
    if let Some(t) = filtered(&sq.authors) {
        clauses.push(group(&t, "AUTH", BoolOp::Or));
    }
    if let Some(t) = filtered(&sq.mesh) {
        clauses.push(group(&t, "MESH", BoolOp::Or));
    }
    if let Some(t) = filtered(&sq.journal) {
        clauses.push(group(&t, "JOURNAL", BoolOp::Or));
    }
    if let Some(t) = filtered(&sq.publication_types) {
        clauses.push(group(&t, "PUB_TYPE", BoolOp::Or));
    }
    if let Some(t) = filtered(&sq.affiliation) {
        clauses.push(group(&t, "AFF", BoolOp::Or));
    }

    if let Some(yr) = &sq.year_range {
        clauses.push(format!("(PUB_YEAR:[{} TO {}])", yr.from, yr.to));
    }

    if clauses.is_empty() {
        return Err(EuropePmcError::Param(
            "structured search is empty: populate at least one field".into(),
        ));
    }

    Ok(clauses.join(" AND "))
}

/// Build a single clause. When `prefix` is empty, terms are emitted bare
/// (all-field search). Multi-term clauses are parenthesised.
fn group(terms: &[String], prefix: &str, op: BoolOp) -> String {
    let tagged: Vec<String> = terms
        .iter()
        .map(|t| {
            let qt = quote_term(t);
            if prefix.is_empty() {
                qt
            } else {
                format!("{prefix}:{qt}")
            }
        })
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

/// Wrap a term in double quotes iff it contains whitespace or special
/// Europe PMC metacharacters. Internal `"` characters are backslash-escaped.
fn quote_term(s: &str) -> String {
    let trimmed = s.trim();
    let needs_quotes = trimmed
        .chars()
        .any(|c| c.is_whitespace() || c == '"' || c == ':');
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

    // ----- single field -----

    #[test]
    fn single_keyword_no_prefix() {
        let sq = StructuredSearch {
            keywords: kw(&["p53"]),
            ..Default::default()
        };
        assert_eq!(to_europepmc(&sq).unwrap(), "p53");
    }

    #[test]
    fn single_keyword_with_space_is_quoted() {
        let sq = StructuredSearch {
            keywords: kw(&["lung cancer"]),
            ..Default::default()
        };
        assert_eq!(to_europepmc(&sq).unwrap(), r#""lung cancer""#);
    }

    #[test]
    fn keywords_default_or() {
        let sq = StructuredSearch {
            keywords: kw(&["cancer", "tumor"]),
            ..Default::default()
        };
        assert_eq!(to_europepmc(&sq).unwrap(), "(cancer OR tumor)");
    }

    #[test]
    fn keywords_explicit_and() {
        let sq = StructuredSearch {
            keywords: kw(&["p53", "cancer"]),
            keywords_op: Some(BoolOp::And),
            ..Default::default()
        };
        assert_eq!(to_europepmc(&sq).unwrap(), "(p53 AND cancer)");
    }

    // ----- field prefixes -----

    #[test]
    fn title_uses_title_prefix() {
        let sq = StructuredSearch {
            title: kw(&["BRCA1", "BRCA2"]),
            ..Default::default()
        };
        assert_eq!(to_europepmc(&sq).unwrap(), "(TITLE:BRCA1 OR TITLE:BRCA2)");
    }

    #[test]
    fn authors_use_auth_prefix() {
        let sq = StructuredSearch {
            authors: kw(&["Smith", "Jones"]),
            ..Default::default()
        };
        assert_eq!(to_europepmc(&sq).unwrap(), "(AUTH:Smith OR AUTH:Jones)");
    }

    #[test]
    fn mesh_uses_mesh_prefix() {
        let sq = StructuredSearch {
            mesh: kw(&["Neoplasms"]),
            ..Default::default()
        };
        assert_eq!(to_europepmc(&sq).unwrap(), "MESH:Neoplasms");
    }

    #[test]
    fn mesh_phrase_is_quoted() {
        let sq = StructuredSearch {
            mesh: kw(&["Breast Neoplasms"]),
            ..Default::default()
        };
        assert_eq!(to_europepmc(&sq).unwrap(), r#"MESH:"Breast Neoplasms""#);
    }

    #[test]
    fn journal_uses_journal_prefix() {
        let sq = StructuredSearch {
            journal: kw(&["Nature"]),
            ..Default::default()
        };
        assert_eq!(to_europepmc(&sq).unwrap(), "JOURNAL:Nature");
    }

    #[test]
    fn pub_type_uses_pub_type_prefix() {
        let sq = StructuredSearch {
            publication_types: kw(&["Review"]),
            ..Default::default()
        };
        assert_eq!(to_europepmc(&sq).unwrap(), "PUB_TYPE:Review");
    }

    #[test]
    fn affiliation_uses_aff_prefix() {
        let sq = StructuredSearch {
            affiliation: kw(&["Harvard Medical School"]),
            ..Default::default()
        };
        assert_eq!(
            to_europepmc(&sq).unwrap(),
            r#"AFF:"Harvard Medical School""#
        );
    }

    // ----- multi-field -----

    #[test]
    fn multiple_fields_joined_with_and() {
        let sq = StructuredSearch {
            keywords: kw(&["p53"]),
            authors: kw(&["Smith"]),
            ..Default::default()
        };
        assert_eq!(to_europepmc(&sq).unwrap(), "p53 AND AUTH:Smith");
    }

    // ----- year range -----

    #[test]
    fn year_range_emits_range_filter() {
        let sq = StructuredSearch {
            keywords: kw(&["cancer"]),
            year_range: Some(YearRange {
                from: 2020,
                to: 2024,
            }),
            ..Default::default()
        };
        assert_eq!(
            to_europepmc(&sq).unwrap(),
            "cancer AND (PUB_YEAR:[2020 TO 2024])"
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
        assert_eq!(to_europepmc(&sq).unwrap(), "(PUB_YEAR:[2024 TO 2024])");
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
        let err = to_europepmc(&sq).unwrap_err();
        assert!(matches!(err, EuropePmcError::Param(_)));
        assert!(format!("{err}").contains("inverted"));
    }

    // ----- errors & edge cases -----

    #[test]
    fn empty_structured_errors() {
        let sq = StructuredSearch::default();
        let err = to_europepmc(&sq).unwrap_err();
        assert!(matches!(err, EuropePmcError::Param(_)));
    }

    #[test]
    fn all_empty_vecs_treated_as_absent() {
        let sq = StructuredSearch {
            keywords: Some(vec!["".into(), "   ".into()]),
            ..Default::default()
        };
        let err = to_europepmc(&sq).unwrap_err();
        assert!(matches!(err, EuropePmcError::Param(_)));
    }

    // ----- quoting -----

    #[test]
    fn quote_term_bare() {
        assert_eq!(quote_term("p53"), "p53");
        assert_eq!(quote_term("  trim  "), "trim");
    }

    #[test]
    fn quote_term_with_space() {
        assert_eq!(quote_term("lung cancer"), r#""lung cancer""#);
    }

    #[test]
    fn quote_term_with_colon() {
        assert_eq!(quote_term("a:b"), r#""a:b""#);
    }
}
