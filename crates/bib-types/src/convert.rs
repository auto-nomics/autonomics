//! Generic conversion utilities for normalising bibliographic data from
//! heterogeneous sources (PubMed ESummary, MEDLINE, CrossRef, GWAS Catalog, …)
//! into [`crate::Article`].
//!
//! These helpers are intentionally source-agnostic. Source-specific parsing
//! (e.g. ESummary JSON → Article) lives in the respective SDK crate.

use crate::types::{Article, ArticleSource, Author, IdKind, Identifier};

// ---------------------------------------------------------------------------
// DOI normalisation
// ---------------------------------------------------------------------------

/// Normalise a DOI string to its canonical bare form.
///
/// Strips URL prefixes (`https://doi.org/`, `doi:`, etc.) and surrounding
/// whitespace.
///
/// ```
/// use bib_types::convert::normalize_doi;
/// assert_eq!(normalize_doi("doi: 10.1000/test"), "10.1000/test");
/// assert_eq!(normalize_doi("https://doi.org/10.1000/test"), "10.1000/test");
/// assert_eq!(normalize_doi("  10.1000/test  "), "10.1000/test");
/// ```
pub fn normalize_doi(raw: &str) -> String {
    let trimmed = raw.trim();
    let stripped = trimmed
        .strip_prefix("https://doi.org/")
        .or_else(|| trimmed.strip_prefix("http://doi.org/"))
        .or_else(|| trimmed.strip_prefix("doi:"))
        .or_else(|| trimmed.strip_prefix("DOI:"))
        .unwrap_or(trimmed);
    stripped.trim().to_owned()
}

// ---------------------------------------------------------------------------
// Publication date parsing
// ---------------------------------------------------------------------------

/// Parse a publication date string like `"2024 Jan 15"` or `"2024 Mar"`
/// into `(year, month)`.
///
/// Returns `(None, None)` if the year cannot be parsed.
///
/// ```
/// use bib_types::convert::parse_pubdate;
/// assert_eq!(parse_pubdate("2024 Jan 15"), (Some(2024u16), Some(1u8)));
/// assert_eq!(parse_pubdate("2024 Mar"), (Some(2024), Some(3)));
/// assert_eq!(parse_pubdate("2024"), (Some(2024), None));
/// assert_eq!(parse_pubdate("nonsense"), (None, None));
/// ```
pub fn parse_pubdate(s: &str) -> (Option<u16>, Option<u8>) {
    let mut parts = s.split_whitespace();
    let year = parts.next().and_then(|y| y.parse::<u16>().ok());
    let month = parts.next().and_then(parse_month);
    (year, month)
}

/// Map a month name or abbreviation to its 1-based number.
fn parse_month(s: &str) -> Option<u8> {
    let lower = s.to_ascii_lowercase();
    let names = [
        "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec",
    ];
    names
        .iter()
        .position(|n| lower.starts_with(n))
        .map(|i| i as u8 + 1)
}

// ---------------------------------------------------------------------------
// Author name parsing
// ---------------------------------------------------------------------------

/// Parse a display-name like `"Smith JA"` or `"Smith John A"` into an
/// [`Author`].
///
/// The convention (from PubMed / MEDLINE `AU` field) is `LastName Initials`,
/// with initials concatenated without spaces (e.g. `"JA"`). If the second
/// token looks like a full name (length > 3 or contains a space), it is
/// stored as `fore_name` instead.
///
/// ```
/// use bib_types::convert::parse_author_name;
///
/// let a = parse_author_name("Smith JA");
/// assert_eq!(a.last_name, "Smith");
/// assert_eq!(a.initials.as_deref(), Some("JA"));
///
/// let b = parse_author_name("van der Berg K");
/// assert_eq!(b.last_name, "van der Berg");
/// ```
pub fn parse_author_name(name: &str) -> Author {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Author {
            last_name: String::new(),
            fore_name: None,
            initials: None,
            affiliation: None,
            orcid: None,
            corresponding: false,
        };
    }

    // PubMed AU format: "LastName Initials" — initials are a run of
    // uppercase letters with no spaces, typically ≤ 5 chars.
    // If the remaining part looks like initials (all caps, short), store as
    // initials. Otherwise treat as fore_name.
    match trimmed.rsplit_once(' ') {
        Some((last, second)) if !last.is_empty() => {
            let looks_like_initials =
                second.len() <= 5 && second.chars().all(|c| c.is_ascii_uppercase());
            if looks_like_initials {
                Author {
                    last_name: last.to_owned(),
                    fore_name: None,
                    initials: Some(second.to_owned()),
                    affiliation: None,
                    orcid: None,
                    corresponding: false,
                }
            } else {
                Author {
                    last_name: last.to_owned(),
                    fore_name: Some(second.to_owned()),
                    initials: None,
                    affiliation: None,
                    orcid: None,
                    corresponding: false,
                }
            }
        }
        _ => Author {
            last_name: trimmed.to_owned(),
            fore_name: None,
            initials: None,
            affiliation: None,
            orcid: None,
            corresponding: false,
        },
    }
}

// ---------------------------------------------------------------------------
// Article builder from common fields
// ---------------------------------------------------------------------------

/// Build an [`Article`] from the minimal set of fields shared across all
/// sources. All other fields start empty/None and can be chained.
#[allow(clippy::too_many_arguments)]
pub fn build_article(
    id: impl Into<String>,
    title: impl Into<String>,
    identifiers: Vec<Identifier>,
    authors: Vec<Author>,
    source: ArticleSource,
) -> Article {
    Article {
        id: id.into(),
        title: title.into(),
        authors,
        identifiers,
        abstract_text: None,
        year: None,
        month: None,
        journal: None,
        volume: None,
        issue: None,
        pages: None,
        issn: None,
        essn: None,
        language: None,
        pub_types: Vec::new(),
        keywords: Vec::new(),
        source,
        created_at: Some(chrono::Utc::now()),
        updated_at: Some(chrono::Utc::now()),
    }
}

/// Convenience: create an [`Identifier`] list from an optional DOI string.
pub fn doi_identifier(raw_doi: Option<&str>) -> Option<Identifier> {
    raw_doi
        .map(normalize_doi)
        .filter(|d| !d.is_empty())
        .map(Identifier::doi)
}

/// Convenience: create a PMID [`Identifier`] from an optional string.
pub fn pmid_identifier(raw_pmid: Option<&str>) -> Option<Identifier> {
    raw_doi_to_id(raw_pmid, IdKind::Pmid)
}

fn raw_doi_to_id(raw: Option<&str>, kind: IdKind) -> Option<Identifier> {
    let v = raw?.trim();
    if v.is_empty() {
        None
    } else {
        Some(Identifier::new(kind, v))
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_doi_variants() {
        assert_eq!(normalize_doi("10.1000/test"), "10.1000/test");
        assert_eq!(normalize_doi("doi: 10.1000/test"), "10.1000/test");
        assert_eq!(normalize_doi("DOI: 10.1000/test"), "10.1000/test");
        assert_eq!(
            normalize_doi("https://doi.org/10.1000/test"),
            "10.1000/test"
        );
        assert_eq!(normalize_doi("  10.1000/test  "), "10.1000/test");
    }

    #[test]
    fn parse_pubdate_variants() {
        assert_eq!(parse_pubdate("2024 Jan 15"), (Some(2024), Some(1)));
        assert_eq!(parse_pubdate("2024 Mar"), (Some(2024), Some(3)));
        assert_eq!(parse_pubdate("2024"), (Some(2024), None));
        assert_eq!(parse_pubdate("2024 Fall"), (Some(2024), None));
        assert_eq!(parse_pubdate(""), (None, None));
    }

    #[test]
    fn parse_author_initials() {
        let a = parse_author_name("Smith JA");
        assert_eq!(a.last_name, "Smith");
        assert_eq!(a.initials.as_deref(), Some("JA"));
        assert!(a.fore_name.is_none());
    }

    #[test]
    fn parse_author_multi_word_last() {
        let a = parse_author_name("van der Berg K");
        assert_eq!(a.last_name, "van der Berg");
        assert_eq!(a.initials.as_deref(), Some("K"));
    }

    #[test]
    fn parse_author_no_initials() {
        let a = parse_author_name("Smith");
        assert_eq!(a.last_name, "Smith");
        assert!(a.initials.is_none());
    }

    #[test]
    fn doi_identifier_normalises() {
        let id = doi_identifier(Some("doi: 10.1000/test")).unwrap();
        assert_eq!(id.kind, IdKind::Doi);
        assert_eq!(id.value, "10.1000/test");
        assert!(doi_identifier(Some("")).is_none());
        assert!(doi_identifier(None).is_none());
    }

    #[test]
    fn pmid_identifier_basic() {
        let id = pmid_identifier(Some("12345")).unwrap();
        assert_eq!(id.kind, IdKind::Pmid);
        assert_eq!(id.value, "12345");
    }

    #[test]
    fn build_article_minimal() {
        let art = build_article(
            "a1",
            "Test",
            vec![Identifier::doi("10.1000/test")],
            vec![],
            ArticleSource::Pubmed,
        );
        assert_eq!(art.title, "Test");
        assert_eq!(art.source, ArticleSource::Pubmed);
        assert!(art.doi().is_some());
    }
}
