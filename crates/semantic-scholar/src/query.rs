//! Semantic Scholar-specific translation of the shared [`StructuredSearch`]
//! model.
//!
//! Unlike Europe PMC, the S2 paper search API does not support field-prefixed
//! query syntax. Instead:
//!
//! - `keywords`, `title`, and `authors` are combined into the plain-text
//!   `query` parameter.
//! - `journal` maps to the `venue` filter.
//! - `publication_types` maps to the `publicationTypes` filter.
//! - `year_range` maps to the `year` filter.
//! - `affiliation` and `mesh` are not natively supported by S2 search, so
//!   they are folded into the text query as additional terms.
//!
//! ## Translation rules
//!
//! - Each text field's terms are joined with the field's boolean operator.
//! - Between fields the join is always a space (implicit AND in S2).
//! - Filters (`venue`, `publicationTypes`, `year`) are returned as
//!   [`S2Filter`] for the caller to pass as query params.

pub use bib_types::query::{BoolOp, StructuredSearch, YearRange};

use crate::client::PaperSearchFilter;
use crate::error::{Result, S2Error};

/// Output of translating a [`StructuredSearch`] into S2 query + filters.
#[derive(Debug, Clone, Default)]
pub struct S2QueryParts {
    /// The plain-text query string.
    pub query: String,
    /// The S2-specific filters.
    pub filter: PaperSearchFilter,
}

/// Translate a [`StructuredSearch`] into S2 query string + filters.
///
/// Returns [`S2Error::Param`] if the query is empty or the year range is
/// inverted.
pub fn to_s2(sq: &StructuredSearch) -> Result<S2QueryParts> {
    if let Some(yr) = &sq.year_range {
        if yr.from > yr.to {
            return Err(S2Error::Param(format!(
                "year_range inverted: from ({}) > to ({})",
                yr.from, yr.to
            )));
        }
    }

    let mut terms: Vec<String> = Vec::new();

    // keywords → bare query terms
    if let Some(t) = filtered(&sq.keywords) {
        let joined = join_terms(&t, sq.keywords_op.unwrap_or_default());
        if !joined.is_empty() {
            terms.push(joined);
        }
    }
    // title → also bare terms (S2 matches title+abstract)
    if let Some(t) = filtered(&sq.title) {
        let joined = join_terms(&t, BoolOp::Or);
        if !joined.is_empty() {
            terms.push(joined);
        }
    }
    // authors → bare terms (S2 search includes author names)
    if let Some(t) = filtered(&sq.authors) {
        let joined = join_terms(&t, BoolOp::Or);
        if !joined.is_empty() {
            terms.push(joined);
        }
    }
    // mesh → S2 doesn't support MeSH in search; add as query terms
    if let Some(t) = filtered(&sq.mesh) {
        let joined = join_terms(&t, BoolOp::Or);
        if !joined.is_empty() {
            terms.push(joined);
        }
    }
    // affiliation → S2 doesn't support affiliation filter; add as query terms
    if let Some(t) = filtered(&sq.affiliation) {
        let joined = join_terms(&t, BoolOp::Or);
        if !joined.is_empty() {
            terms.push(joined);
        }
    }

    if terms.is_empty()
        && sq.journal.is_none()
        && sq.publication_types.is_none()
        && sq.year_range.is_none()
    {
        return Err(S2Error::Param(
            "structured search is empty: populate at least one field".into(),
        ));
    }

    let query = terms.join(" ");

    let mut filter = PaperSearchFilter::default();

    // journal → venue filter
    if let Some(t) = filtered(&sq.journal) {
        filter.venue = Some(t.join(","));
    }

    // publication_types → publicationTypes filter
    if let Some(t) = filtered(&sq.publication_types) {
        filter.publication_types = Some(t.join(","));
    }

    // year_range → year filter
    if let Some(yr) = &sq.year_range {
        filter.year = Some(if yr.from == yr.to {
            yr.from.to_string()
        } else {
            format!("{}-{}", yr.from, yr.to)
        });
    }

    Ok(S2QueryParts { query, filter })
}

/// Whether this query consists solely of keyword terms.
///
/// The bulk-search endpoint supports Boolean query syntax but not the relevance
/// endpoint's field filters, so this lets callers select the correct endpoint.
pub fn is_keywords_only(sq: &StructuredSearch) -> bool {
    sq.keywords.is_some()
        && sq.title.is_none()
        && sq.authors.is_none()
        && sq.mesh.is_none()
        && sq.journal.is_none()
        && sq.publication_types.is_none()
        && sq.affiliation.is_none()
        && sq.year_range.is_none()
}

/// Join terms with the given boolean operator. Terms with spaces are
/// phrase-quoted.
fn join_terms(terms: &[String], op: BoolOp) -> String {
    let quoted: Vec<String> = terms.iter().map(|t| quote_term(t)).collect();
    let sep = match op {
        BoolOp::And => " ",
        BoolOp::Or => " | ",
        BoolOp::Not => " -",
    };
    quoted.join(sep)
}

/// Return a trimmed copy of the field's terms, dropping empties.
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

/// Quote a term if it contains whitespace.
fn quote_term(s: &str) -> String {
    let trimmed = s.trim();
    if trimmed.chars().any(|c| c.is_whitespace()) {
        format!("\"{trimmed}\"")
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

    #[test]
    fn single_keyword() {
        let sq = StructuredSearch {
            keywords: kw(&["p53"]),
            ..Default::default()
        };
        let parts = to_s2(&sq).unwrap();
        assert_eq!(parts.query, "p53");
    }

    #[test]
    fn keywords_default_or() {
        let sq = StructuredSearch {
            keywords: kw(&["p53", "cancer"]),
            ..Default::default()
        };
        let parts = to_s2(&sq).unwrap();
        assert_eq!(parts.query, "p53 | cancer");
    }

    #[test]
    fn keywords_explicit_or() {
        let sq = StructuredSearch {
            keywords: kw(&["p53", "cancer"]),
            keywords_op: Some(BoolOp::Or),
            ..Default::default()
        };
        let parts = to_s2(&sq).unwrap();
        assert_eq!(parts.query, "p53 | cancer");
    }

    #[test]
    fn keyword_with_space_is_quoted() {
        let sq = StructuredSearch {
            keywords: kw(&["lung cancer"]),
            ..Default::default()
        };
        let parts = to_s2(&sq).unwrap();
        assert_eq!(parts.query, "\"lung cancer\"");
    }

    #[test]
    fn year_range_becomes_filter() {
        let sq = StructuredSearch {
            keywords: kw(&["cancer"]),
            year_range: Some(YearRange {
                from: 2020,
                to: 2024,
            }),
            ..Default::default()
        };
        let parts = to_s2(&sq).unwrap();
        assert_eq!(parts.query, "cancer");
        assert_eq!(parts.filter.year.as_deref(), Some("2020-2024"));
    }

    #[test]
    fn journal_becomes_venue_filter() {
        let sq = StructuredSearch {
            keywords: kw(&["cancer"]),
            journal: kw(&["Nature"]),
            ..Default::default()
        };
        let parts = to_s2(&sq).unwrap();
        assert_eq!(parts.filter.venue.as_deref(), Some("Nature"));
    }

    #[test]
    fn pub_type_becomes_filter() {
        let sq = StructuredSearch {
            keywords: kw(&["cancer"]),
            publication_types: kw(&["Review", "JournalArticle"]),
            ..Default::default()
        };
        let parts = to_s2(&sq).unwrap();
        assert_eq!(
            parts.filter.publication_types.as_deref(),
            Some("Review,JournalArticle")
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
        assert!(to_s2(&sq).is_err());
    }

    #[test]
    fn empty_structured_errors() {
        let sq = StructuredSearch::default();
        assert!(to_s2(&sq).is_err());
    }

    #[test]
    fn multiple_text_fields_joined() {
        let sq = StructuredSearch {
            keywords: kw(&["cancer"]),
            authors: kw(&["Smith"]),
            ..Default::default()
        };
        let parts = to_s2(&sq).unwrap();
        assert_eq!(parts.query, "cancer Smith");
    }

    #[test]
    fn keywords_only_detection() {
        let sq = StructuredSearch {
            keywords: kw(&["p53", "cancer"]),
            ..Default::default()
        };
        assert!(is_keywords_only(&sq));

        let sq = StructuredSearch {
            keywords: kw(&["p53"]),
            year_range: Some(YearRange {
                from: 2020,
                to: 2024,
            }),
            ..Default::default()
        };
        assert!(!is_keywords_only(&sq));
    }
}
