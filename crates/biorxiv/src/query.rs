//! bioRxiv/medRxiv translation of the shared [`StructuredSearch`] model.
//!
//! Unlike PubMed, Embase, or arXiv, the bioRxiv/medRxiv API has **no
//! keyword or field-level search**. It only supports browsing by date range
//! or recency. Therefore [`to_biorxiv`] cannot produce a server-side query
//! string — instead it returns a [`BiorxivQuery`] comprising:
//!
//! 1. An [`Interval`] for the API call (derived from `year_range`).
//! 2. A set of [`Filter`] predicates that the caller applies **client-side**
//!    after fetching.
//!
//! ## Translation rules
//!
//! - `year_range` → [`Interval::DateRange`] (Jan 1 of `from` – Dec 31 of `to`).
//! - If no `year_range`, the caller must supply a default interval
//!   (e.g. last 30 days).
//! - `keywords` → match against title **and** abstract (case-insensitive).
//! - `title` → match against title only.
//! - `authors` → match against the `authors` string.
//! - Other fields (`journal`, `mesh`, `affiliation`, `publication_types`)
//!   are best-effort: keywords filter on title/abstract, types on category.
//!
//! ## Example
//!
//! ```
//! use bib_types::query::{BoolOp, StructuredSearch, YearRange};
//!
//! let sq = StructuredSearch {
//!     keywords: Some(vec!["CRISPR".into()]),
//!     year_range: Some(YearRange { from: 2023, to: 2024 }),
//!     ..Default::default()
//! };
//! let query = biorxiv::query::to_biorxiv(sq, biorxiv::types::Server::Medrxiv).unwrap();
//! assert!(query.interval.path().contains("2023-01-01"));
//! assert!(!query.filters.is_empty());
//! ```

// Re-export the shared model types.
pub use bib_types::query::{BoolOp, StructuredSearch, YearRange};

use chrono::NaiveDate;

use crate::error::{BiorxivError, Result};
use crate::types::{BiorxivEntry, Interval, Server};

// ---------------------------------------------------------------------------
// BiorxivQuery — the translated result
// ---------------------------------------------------------------------------

/// A translated bioRxiv/medRxiv query: API interval + client-side filters.
#[derive(Debug, Clone)]
pub struct BiorxivQuery {
    /// Which server to query.
    pub server: Server,
    /// Date interval for the API call.
    pub interval: Interval,
    /// Client-side filter predicates.
    pub filters: Vec<Filter>,
}

impl BiorxivQuery {
    /// `true` if there are no client-side filters (pure date browse).
    pub fn is_unfiltered(&self) -> bool {
        self.filters.is_empty()
    }

    /// Apply all filters to an entry. Returns `true` if the entry passes.
    pub fn matches(&self, entry: &BiorxivEntry) -> bool {
        self.filters.iter().all(|f| f.matches(entry))
    }
}

/// A single client-side filter predicate.
#[derive(Debug, Clone)]
pub enum Filter {
    /// Match any/all terms against title + abstract.
    Keywords { terms: Vec<String>, op: BoolOp },
    /// Match any term against title only.
    Title(Vec<String>),
    /// Match any author name against the authors string.
    Authors(Vec<String>),
}

impl Filter {
    /// Evaluate the filter against an entry.
    fn matches(&self, entry: &BiorxivEntry) -> bool {
        match self {
            Filter::Keywords { terms, op } => {
                let hay = format!("{} {}", entry.title, entry.abstract_text).to_ascii_lowercase();
                evaluate_terms(terms, op, &hay)
            }
            Filter::Title(terms) => {
                let hay = entry.title.to_ascii_lowercase();
                terms.iter().any(|t| hay.contains(&t.to_ascii_lowercase()))
            }
            Filter::Authors(terms) => {
                let hay = entry.authors.to_ascii_lowercase();
                terms.iter().any(|t| hay.contains(&t.to_ascii_lowercase()))
            }
        }
    }
}

/// Evaluate a list of terms against a haystack using the given boolean op.
fn evaluate_terms(terms: &[String], op: &BoolOp, hay: &str) -> bool {
    match op {
        BoolOp::And => terms.iter().all(|t| hay.contains(&t.to_ascii_lowercase())),
        BoolOp::Not => terms.iter().all(|t| !hay.contains(&t.to_ascii_lowercase())),
        BoolOp::Or => terms.iter().any(|t| hay.contains(&t.to_ascii_lowercase())),
    }
}

// ---------------------------------------------------------------------------
// Translation
// ---------------------------------------------------------------------------

/// Translate a [`StructuredSearch`] into a [`BiorxivQuery`].
///
/// **Important**: the bioRxiv/medRxiv API has no keyword search. Keywords,
/// title, and author terms become **client-side filters** — the caller must
/// fetch results by `interval` and then filter with [`BiorxivQuery::matches`].
///
/// If no `year_range` is set, the interval defaults to the last 365 days.
pub fn to_biorxiv(sq: StructuredSearch, server: Server) -> Result<BiorxivQuery> {
    // Validate year range.
    if let Some(yr) = &sq.year_range {
        if yr.from > yr.to {
            return Err(BiorxivError::Param(format!(
                "year_range inverted: from ({}) > to ({})",
                yr.from, yr.to
            )));
        }
    }

    // Determine interval.
    let interval = match &sq.year_range {
        Some(yr) => Interval::DateRange {
            from: NaiveDate::from_ymd_opt(yr.from as i32, 1, 1)
                .ok_or_else(|| BiorxivError::Param("invalid from year".into()))?,
            to: NaiveDate::from_ymd_opt(yr.to as i32, 12, 31)
                .ok_or_else(|| BiorxivError::Param("invalid to year".into()))?,
        },
        None => Interval::Days(365),
    };

    // Build client-side filters.
    let mut filters = Vec::new();

    if let Some(terms) = filter_nonempty(&sq.keywords) {
        let op = sq.keywords_op.unwrap_or_default();
        filters.push(Filter::Keywords { terms, op });
    }

    if let Some(terms) = filter_nonempty(&sq.title) {
        filters.push(Filter::Title(terms));
    }

    if let Some(terms) = filter_nonempty(&sq.authors) {
        filters.push(Filter::Authors(terms));
    }

    // Fields without direct API support fall back to keyword-style filtering.
    if let Some(terms) = filter_nonempty(&sq.publication_types) {
        filters.push(Filter::Keywords {
            terms,
            op: BoolOp::Or,
        });
    }

    Ok(BiorxivQuery {
        server,
        interval,
        filters,
    })
}

/// Return a cleaned, non-empty Vec of terms, or `None` if all blank.
fn filter_nonempty(opt: &Option<Vec<String>>) -> Option<Vec<String>> {
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
    use crate::types::BiorxivEntry;

    fn entry() -> BiorxivEntry {
        BiorxivEntry {
            doi: "10.1101/2024.01.01.1".into(),
            title: "CRISPR Gene Editing in Cancer Cells".into(),
            authors: "Smith, J.; Doe, K.".into(),
            abstract_text: "We used CRISPR to study cancer pathways.".into(),
            ..Default::default()
        }
    }

    #[test]
    fn year_range_to_interval() {
        let sq = StructuredSearch {
            year_range: Some(YearRange {
                from: 2023,
                to: 2024,
            }),
            ..Default::default()
        };
        let q = to_biorxiv(sq, Server::Medrxiv).unwrap();
        assert!(q.interval.path().contains("2023-01-01"));
        assert!(q.interval.path().contains("2024-12-31"));
        assert!(q.is_unfiltered());
    }

    #[test]
    fn no_year_defaults_to_days() {
        let sq = StructuredSearch {
            keywords: Some(vec!["cancer".into()]),
            ..Default::default()
        };
        let q = to_biorxiv(sq, Server::Biorxiv).unwrap();
        assert_eq!(q.interval, Interval::Days(365));
    }

    #[test]
    fn keywords_filter() {
        let sq = StructuredSearch {
            keywords: Some(vec!["CRISPR".into()]),
            ..Default::default()
        };
        let q = to_biorxiv(sq, Server::Medrxiv).unwrap();
        assert!(q.matches(&entry()));
    }

    #[test]
    fn keywords_and_not_match() {
        let sq = StructuredSearch {
            keywords: Some(vec!["CRISPR".into(), "banana".into()]),
            keywords_op: Some(BoolOp::And),
            ..Default::default()
        };
        let q = to_biorxiv(sq, Server::Medrxiv).unwrap();
        assert!(!q.matches(&entry())); // "banana" not in entry
    }

    #[test]
    fn keywords_or_match() {
        let sq = StructuredSearch {
            keywords: Some(vec!["CRISPR".into(), "banana".into()]),
            keywords_op: Some(BoolOp::Or),
            ..Default::default()
        };
        let q = to_biorxiv(sq, Server::Medrxiv).unwrap();
        assert!(q.matches(&entry()));
    }

    #[test]
    fn title_filter() {
        let sq = StructuredSearch {
            title: Some(vec!["Cancer".into()]),
            ..Default::default()
        };
        let q = to_biorxiv(sq, Server::Medrxiv).unwrap();
        assert!(q.matches(&entry()));

        let sq2 = StructuredSearch {
            title: Some(vec!["banana".into()]),
            ..Default::default()
        };
        let q2 = to_biorxiv(sq2, Server::Medrxiv).unwrap();
        assert!(!q2.matches(&entry()));
    }

    #[test]
    fn author_filter() {
        let sq = StructuredSearch {
            authors: Some(vec!["Smith".into()]),
            ..Default::default()
        };
        let q = to_biorxiv(sq, Server::Medrxiv).unwrap();
        assert!(q.matches(&entry()));
    }

    #[test]
    fn combined_filters_and() {
        let sq = StructuredSearch {
            keywords: Some(vec!["CRISPR".into()]),
            authors: Some(vec!["Smith".into()]),
            ..Default::default()
        };
        let q = to_biorxiv(sq, Server::Medrxiv).unwrap();
        assert!(q.matches(&entry())); // both match

        let sq2 = StructuredSearch {
            keywords: Some(vec!["banana".into()]),
            authors: Some(vec!["Smith".into()]),
            ..Default::default()
        };
        let q2 = to_biorxiv(sq2, Server::Medrxiv).unwrap();
        assert!(!q2.matches(&entry())); // keyword doesn't match
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
        let err = to_biorxiv(sq, Server::Medrxiv).unwrap_err();
        assert!(format!("{err}").contains("inverted"));
    }

    #[test]
    fn empty_filters_produce_unfiltered_query() {
        let sq = StructuredSearch::default();
        let q = to_biorxiv(sq, Server::Medrxiv).unwrap();
        assert!(q.is_unfiltered());
    }
}
