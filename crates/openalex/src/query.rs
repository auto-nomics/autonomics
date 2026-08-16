//! Translation of the shared [`StructuredSearch`] model into OpenAlex filter
//! syntax.
//!
//! OpenAlex uses a `filter` query parameter with `field:value` pairs joined by
//! commas (AND). OR within a field uses the pipe character: `type:article|book`.
//!
//! ## Translation rules
//!
//! - Each populated field produces one `key:value` filter clause.
//! - Between fields the join is `,` (AND in OpenAlex).
//! - Within a multi-term field the join defaults to `|` (OR); `keywords`
//!   honours an explicit [`BoolOp`].
//! - Year range maps to `from_publication_date` / `to_publication_date`.

// Re-export the shared model types.
pub use bib_types::query::{BoolOp, StructuredSearch, YearRange};

use crate::error::{OpenAlexError, Result};

/// Translate a [`StructuredSearch`] into an OpenAlex filter string.
pub fn to_openalex_filter(sq: &StructuredSearch) -> Result<String> {
    if let Some(yr) = &sq.year_range {
        if yr.from > yr.to {
            return Err(OpenAlexError::Param(format!(
                "year_range inverted: from ({}) > to ({})",
                yr.from, yr.to
            )));
        }
    }

    let mut clauses: Vec<String> = Vec::new();

    // keywords → full-text search (search param, not filter)
    // We return keywords separately so the caller can pass it as `search`.
    // For filter, we map title/authors/etc. below.

    // title → title.search:"term"
    if let Some(t) = filtered(&sq.title) {
        clauses.push(pipe_join("title.search", &t));
    }

    // authors → author.display_name.search:"name" (use authorships.author.display_name.search)
    if let Some(t) = filtered(&sq.authors) {
        clauses.push(pipe_join("author.display_name.search", &t));
    }

    // mesh → concepts.wikidata or just title/abstract search; OpenAlex doesn't have
    // a direct MeSH filter but it does index MeSH via `concepts.id`.
    // We use `fulltext.search` as a fallback.
    if let Some(t) = filtered(&sq.mesh) {
        clauses.push(pipe_join("fulltext.search", &t));
    }

    // journal → primary_location.source.display_name.search
    if let Some(t) = filtered(&sq.journal) {
        clauses.push(pipe_join("primary_location.source.display_name.search", &t));
    }

    // publication_types → type
    if let Some(t) = filtered(&sq.publication_types) {
        let lower: Vec<String> = t.iter().map(|s| s.to_lowercase()).collect();
        clauses.push(pipe_join("type", &lower));
    }

    // affiliation → fulltext.search
    if let Some(t) = filtered(&sq.affiliation) {
        clauses.push(pipe_join("fulltext.search", &t));
    }

    // year range → from_publication_date / to_publication_date
    if let Some(yr) = &sq.year_range {
        clauses.push(format!("from_publication_date:{}-01-01", yr.from));
        clauses.push(format!("to_publication_date:{}-12-31", yr.to));
    }

    if clauses.is_empty() && sq.keywords.is_none() {
        return Err(OpenAlexError::Param(
            "structured search is empty: populate at least one field".into(),
        ));
    }

    Ok(clauses.join(","))
}

/// Extract the keywords portion of a [`StructuredSearch`] as a search query.
///
/// OpenAlex search treats whitespace as AND and `|` as OR. Returns `None` if
/// keywords are absent.
pub fn keywords_search(sq: &StructuredSearch) -> Option<String> {
    let terms = filtered(&sq.keywords)?;
    let op = sq.keywords_op.unwrap_or_default();
    let quoted: Vec<String> = terms.iter().map(|t| quote_term(t)).collect();
    Some(match op {
        BoolOp::Or => quoted.join("|"),
        BoolOp::And => quoted.join(" "),
        BoolOp::Not => {
            let mut query = quoted;
            for term in query.iter_mut().skip(1) {
                *term = format!("-{term}");
            }
            query.join(" ")
        }
    })
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Join terms with OpenAlex OR-pipe syntax: `key:val1|val2|val3`.
fn pipe_join(key: &str, terms: &[String]) -> String {
    let quoted: Vec<String> = terms.iter().map(|t| quote_term(t)).collect();
    format!("{}:{}", key, quoted.join("|"))
}

/// Return a trimmed copy of the field's terms, dropping any that are empty.
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

/// Wrap a term in double quotes iff it contains whitespace or special chars.
fn quote_term(s: &str) -> String {
    let trimmed = s.trim();
    let needs_quotes = trimmed
        .chars()
        .any(|c| c.is_whitespace() || c == '"' || c == ':' || c == '|');
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

    #[test]
    fn title_filter() {
        let sq = StructuredSearch {
            title: kw(&["cancer"]),
            ..Default::default()
        };
        assert_eq!(to_openalex_filter(&sq).unwrap(), "title.search:cancer");
    }

    #[test]
    fn title_multi_or() {
        let sq = StructuredSearch {
            title: kw(&["cancer", "tumor"]),
            ..Default::default()
        };
        assert_eq!(
            to_openalex_filter(&sq).unwrap(),
            "title.search:cancer|tumor"
        );
    }

    #[test]
    fn authors_filter() {
        let sq = StructuredSearch {
            authors: kw(&["Smith", "Jones"]),
            ..Default::default()
        };
        let f = to_openalex_filter(&sq).unwrap();
        assert!(f.contains("author.display_name.search:Smith|Jones"));
    }

    #[test]
    fn year_range_filter() {
        let sq = StructuredSearch {
            year_range: Some(YearRange {
                from: 2020,
                to: 2024,
            }),
            ..Default::default()
        };
        let f = to_openalex_filter(&sq).unwrap();
        assert!(f.contains("from_publication_date:2020-01-01"));
        assert!(f.contains("to_publication_date:2024-12-31"));
    }

    #[test]
    fn pub_type_lowercased() {
        let sq = StructuredSearch {
            publication_types: kw(&["Review", "Article"]),
            ..Default::default()
        };
        let f = to_openalex_filter(&sq).unwrap();
        assert!(f.contains("type:review|article"));
    }

    #[test]
    fn keywords_search_and_is_space_joined() {
        let sq = StructuredSearch {
            keywords: kw(&["p53", "cancer"]),
            keywords_op: Some(BoolOp::And),
            ..Default::default()
        };
        assert_eq!(keywords_search(&sq).unwrap(), "p53 cancer");
    }

    #[test]
    fn keywords_search_or_uses_pipe() {
        let sq = StructuredSearch {
            keywords: kw(&["p53", "cancer"]),
            keywords_op: Some(BoolOp::Or),
            ..Default::default()
        };
        assert_eq!(keywords_search(&sq).unwrap(), "p53|cancer");
    }

    #[test]
    fn keywords_search_quotes_phrases() {
        let sq = StructuredSearch {
            keywords: kw(&["lung cancer", "biomarker"]),
            keywords_op: Some(BoolOp::And),
            ..Default::default()
        };
        assert_eq!(keywords_search(&sq).unwrap(), "\"lung cancer\" biomarker");
    }

    #[test]
    fn empty_errors() {
        let sq = StructuredSearch::default();
        let err = to_openalex_filter(&sq).unwrap_err();
        assert!(matches!(err, OpenAlexError::Param(_)));
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
        let err = to_openalex_filter(&sq).unwrap_err();
        assert!(format!("{err}").contains("inverted"));
    }
}
