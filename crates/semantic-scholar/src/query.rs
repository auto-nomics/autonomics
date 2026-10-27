//! Semantic Scholar-specific translation of the shared [`StructuredSearch`]
//! model.
//!
//! Keywords use the bulk endpoint's Boolean grammar; venue, type and years
//! remain metadata filters. Title-only, author, MeSH and affiliation constraints
//! cannot be expressed by this endpoint and are rejected, never folded into
//! title/abstract text. Use discovery's explicit approximate mode or native API
//! calls when broader text matching is intended.

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
        if yr.from == 0 || yr.to > 9999 {
            return Err(S2Error::Param(
                "publication years must be between 1 and 9999".into(),
            ));
        }
        if yr.from > yr.to {
            return Err(S2Error::Param(format!(
                "year_range inverted: from ({}) > to ({})",
                yr.from, yr.to
            )));
        }
    }

    for (name, values) in [
        ("title", &sq.title),
        ("author", &sq.authors),
        ("MeSH", &sq.mesh),
        ("affiliation", &sq.affiliation),
    ] {
        if filtered(values).is_some() {
            return Err(S2Error::Param(format!(
                "Semantic Scholar cannot preserve a {name} constraint in bulk search; use explicit discovery approximation or another source"
            )));
        }
    }
    let mut terms: Vec<String> = Vec::new();
    if let Some(values) = filtered(&sq.keywords) {
        let op = sq.keywords_op.unwrap_or_default();
        if op == BoolOp::Not && values.len() < 2 {
            return Err(S2Error::Param(
                "exclusions require a positive keyword anchor".into(),
            ));
        }
        let values = values
            .iter()
            .map(|value| quote_term(value))
            .collect::<Result<Vec<_>>>()?;
        terms.push(match op {
            BoolOp::And => values.join(" + "),
            BoolOp::Or => values.join(" | "),
            BoolOp::Not => values.join(" -"),
        });
    }
    if terms.is_empty()
        && filtered(&sq.journal).is_none()
        && filtered(&sq.publication_types).is_none()
        && sq.year_range.is_none()
    {
        return Err(S2Error::Param(
            "structured search is empty: populate a supported field".into(),
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
/// Retained for compatibility; the bulk endpoint also supports metadata filters.
/// Boolean translations must always use bulk, even when this returns false.
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
fn quote_term(s: &str) -> Result<String> {
    let trimmed = s.trim();
    if trimmed
        .chars()
        .any(|ch| ch.is_control() || matches!(ch, '"' | '\\' | '*' | '~' | '?'))
    {
        return Err(S2Error::Param(
            "keyword contains source syntax; use native API explicitly".into(),
        ));
    }
    Ok(
        if trimmed
            .chars()
            .any(|ch| !ch.is_alphanumeric() && ch != '_' && ch != '.')
        {
            format!("\"{trimmed}\"")
        } else {
            trimmed.into()
        },
    )
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
    fn unsupported_author_is_not_folded_into_text() {
        let sq = StructuredSearch {
            keywords: kw(&["cancer"]),
            authors: kw(&["Smith"]),
            ..Default::default()
        };
        assert!(matches!(to_s2(&sq), Err(S2Error::Param(_))));
    }

    #[test]
    fn unsupported_fields_empty_vectors_and_single_not_never_widen_search() {
        for name in ["title", "mesh", "affiliation"] {
            let mut sq = StructuredSearch {
                keywords: kw(&["gene"]),
                ..Default::default()
            };
            match name {
                "title" => sq.title = kw(&["special"]),
                "mesh" => sq.mesh = kw(&["Asthma"]),
                _ => sq.affiliation = kw(&["Lab"]),
            }
            assert!(matches!(to_s2(&sq), Err(S2Error::Param(_))));
        }
        let mut sq = StructuredSearch {
            keywords: kw(&[" "]),
            journal: Some(vec![]),
            ..Default::default()
        };
        assert!(to_s2(&sq).is_err());
        sq.keywords = kw(&["mice"]);
        sq.keywords_op = Some(BoolOp::Not);
        assert!(to_s2(&sq).is_err());
        sq.keywords = kw(&["gene", "mice"]);
        assert_eq!(to_s2(&sq).unwrap().query, "gene -mice");
        sq.keywords = kw(&["gene*", "mice"]);
        assert!(to_s2(&sq).is_err());
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
