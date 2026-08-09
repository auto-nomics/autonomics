//! Crossref-specific translation of the shared [`StructuredSearch`] model.
//!
//! The query model itself ([`StructuredSearch`], [`BoolOp`], [`YearRange`])
//! lives in `bib-types` so that every SDK in the workspace shares the same
//! typed search interface. This module provides [`to_crossref_works_query`],
//! the function that renders a `StructuredSearch` into a [`WorksQuery`] for
//! the Crossref `/works` endpoint.
//!
//! ## Crossref field-query parameters
//!
//! | Field           | Param                   | Example                    |
//! |-----------------|-------------------------|----------------------------|
//! | All fields      | `query`                 | `query=p53+cancer`         |
//! | Title/biblio    | `query.bibliographic`   | `query.bibliographic=room` |
//! | Author          | `query.author`          | `query.author=smith`       |
//! | Container title | `query.container-title` | `query.container-title=Nature` |
//! | Affiliation     | `query.affiliation`     | `query.affiliation=Harvard`|
//!
//! Crossref does NOT have MeSH or publication-type field queries — those map
//! to filters instead.
//!
//! ## Translation rules
//!
//! - `keywords` → free-form `query` param (all fields).
//! - `title` → `query.bibliographic`.
//! - `authors` → `query.author`.
//! - `journal` → `query.container-title`.
//! - `affiliation` → `query.affiliation`.
//! - `mesh` → no Crossref equivalent (silently dropped; could use
//!   `category-name` filter if needed).
//! - `publication_types` → `type` filter.
//! - `year_range` → `from-pub-date` / `until-pub-date` filters.

// Re-export shared model types.
pub use bib_types::query::{BoolOp, StructuredSearch, YearRange};

use crate::client::WorksQuery;
use crate::error::{CrossrefError, Result};

/// Translate a [`StructuredSearch`] into a [`WorksQuery`] for the
/// `/works` endpoint.
pub fn to_crossref_works_query(sq: &StructuredSearch) -> Result<WorksQuery> {
    if let Some(yr) = &sq.year_range {
        if yr.from > yr.to {
            return Err(CrossrefError::Param(format!(
                "year_range inverted: from ({}) > to ({})",
                yr.from, yr.to
            )));
        }
    }

    let mut q = WorksQuery::new();

    // keywords → free-form query (all fields)
    if let Some(t) = filtered(&sq.keywords) {
        let op = sq.keywords_op.unwrap_or_default();
        q = q.with_query(join_terms(&t, op));
    }

    // title → query.bibliographic (best available for title search)
    if let Some(t) = filtered(&sq.title) {
        q = q.with_field_query("query.bibliographic", join_terms(&t, BoolOp::Or));
    }

    // authors → query.author
    if let Some(t) = filtered(&sq.authors) {
        q = q.with_field_query("query.author", join_terms(&t, BoolOp::Or));
    }

    // journal → query.container-title
    if let Some(t) = filtered(&sq.journal) {
        q = q.with_field_query("query.container-title", join_terms(&t, BoolOp::Or));
    }

    // affiliation → query.affiliation
    if let Some(t) = filtered(&sq.affiliation) {
        q = q.with_field_query("query.affiliation", join_terms(&t, BoolOp::Or));
    }

    // publication_types → type filter
    if let Some(t) = filtered(&sq.publication_types) {
        for pt in &t {
            q = q.with_filter("type", pt.clone());
        }
    }

    // year_range → date filters
    if let Some(yr) = &sq.year_range {
        q = q.with_filter("from-pub-date", format!("{}-01-01", yr.from));
        q = q.with_filter("until-pub-date", format!("{}-12-31", yr.to));
    }

    // Check that at least one constraint was set.
    let is_empty = sq.keywords.is_none()
        && sq.title.is_none()
        && sq.authors.is_none()
        && sq.journal.is_none()
        && sq.affiliation.is_none()
        && sq.publication_types.is_none()
        && sq.year_range.is_none();
    if is_empty {
        return Err(CrossrefError::Param(
            "structured search is empty: populate at least one field".into(),
        ));
    }

    Ok(q)
}

// -----------------------------------------------------------------------
// Helpers
// -----------------------------------------------------------------------

/// Join terms with the given operator for Crossref's free-form query.
/// Crossref uses `+` for AND and spaces for OR by default, but we use
/// explicit boolean words which the API also supports in query params.
fn join_terms(terms: &[String], op: BoolOp) -> String {
    let parts: Vec<String> = terms.iter().map(|t| quote_if_needed(t)).collect();
    match op {
        BoolOp::Not => {
            // NOT doesn't make sense as a sole join; emit as AND-NOT.
            let first = parts.first().cloned().unwrap_or_default();
            let rest: Vec<String> = parts.iter().skip(1).map(|t| format!("NOT {t}")).collect();
            [first]
                .into_iter()
                .chain(rest)
                .collect::<Vec<_>>()
                .join(" AND ")
        }
        _ => parts.join(&format!(" {} ", op.keyword())),
    }
}

/// Quote a term if it contains whitespace.
fn quote_if_needed(s: &str) -> String {
    let trimmed = s.trim();
    if trimmed.chars().any(|c| c.is_whitespace()) {
        format!("\"{trimmed}\"")
    } else {
        trimmed.to_string()
    }
}

/// Return a trimmed copy of the field's terms, dropping empty entries.
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
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_keyword() {
        let sq = StructuredSearch {
            keywords: Some(vec!["p53".into()]),
            ..Default::default()
        };
        let q = to_crossref_works_query(&sq).unwrap();
        assert_eq!(q.query.as_deref(), Some("p53"));
    }

    #[test]
    fn keywords_default_or() {
        let sq = StructuredSearch {
            keywords: Some(vec!["cancer".into(), "tumor".into()]),
            ..Default::default()
        };
        let q = to_crossref_works_query(&sq).unwrap();
        assert_eq!(q.query.as_deref(), Some("cancer OR tumor"));
    }

    #[test]
    fn keywords_explicit_and() {
        let sq = StructuredSearch {
            keywords: Some(vec!["p53".into(), "cancer".into()]),
            keywords_op: Some(BoolOp::And),
            ..Default::default()
        };
        let q = to_crossref_works_query(&sq).unwrap();
        assert_eq!(q.query.as_deref(), Some("p53 AND cancer"));
    }

    #[test]
    fn title_maps_to_bibliographic() {
        let sq = StructuredSearch {
            title: Some(vec!["CRISPR".into()]),
            ..Default::default()
        };
        let q = to_crossref_works_query(&sq).unwrap();
        assert!(q.query.is_none());
        assert!(
            q.field_queries
                .iter()
                .any(|(f, _)| f == "query.bibliographic")
        );
    }

    #[test]
    fn authors_maps_to_field_query() {
        let sq = StructuredSearch {
            authors: Some(vec!["Smith".into(), "Jones".into()]),
            ..Default::default()
        };
        let q = to_crossref_works_query(&sq).unwrap();
        let (_, v) = q
            .field_queries
            .iter()
            .find(|(f, _)| f == "query.author")
            .unwrap();
        assert_eq!(v, "Smith OR Jones");
    }

    #[test]
    fn journal_maps_to_container_title() {
        let sq = StructuredSearch {
            journal: Some(vec!["Nature".into()]),
            ..Default::default()
        };
        let q = to_crossref_works_query(&sq).unwrap();
        assert!(
            q.field_queries
                .iter()
                .any(|(f, _)| f == "query.container-title")
        );
    }

    #[test]
    fn publication_type_maps_to_type_filter() {
        let sq = StructuredSearch {
            publication_types: Some(vec!["journal-article".into()]),
            ..Default::default()
        };
        let q = to_crossref_works_query(&sq).unwrap();
        assert!(
            q.filters
                .iter()
                .any(|(k, v)| k == "type" && v == "journal-article")
        );
    }

    #[test]
    fn year_range_maps_to_date_filters() {
        let sq = StructuredSearch {
            keywords: Some(vec!["cancer".into()]),
            year_range: Some(YearRange {
                from: 2020,
                to: 2024,
            }),
            ..Default::default()
        };
        let q = to_crossref_works_query(&sq).unwrap();
        assert!(
            q.filters
                .iter()
                .any(|(k, v)| k == "from-pub-date" && v == "2020-01-01")
        );
        assert!(
            q.filters
                .iter()
                .any(|(k, v)| k == "until-pub-date" && v == "2024-12-31")
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
        let err = to_crossref_works_query(&sq).unwrap_err();
        assert!(matches!(err, CrossrefError::Param(_)));
        assert!(format!("{err}").contains("inverted"));
    }

    #[test]
    fn empty_structured_errors() {
        let sq = StructuredSearch::default();
        let err = to_crossref_works_query(&sq).unwrap_err();
        assert!(matches!(err, CrossrefError::Param(_)));
    }

    #[test]
    fn join_terms_not_operator() {
        let terms = vec!["cancer".into(), "tumor".into()];
        let joined = join_terms(&terms, BoolOp::Not);
        assert!(joined.contains("NOT tumor"));
    }
}
