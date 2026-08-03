//! Conversion layer: parse Embase JSON responses into typed
//! [`bib_types::Article`] records.
//!
//! The Embase API returns Atom-style JSON with Dublin Core (`dc:`) and
//! PRISM (`prism:`) namespaced fields. This module converts them into the
//! canonical [`Article`] model.
//!
//! ## Search response
//!
//! ```json
//! {
//!   "opensearch:totalResults": "12345",
//!   "opensearch:startIndex": "1",
//!   "opensearch:itemsPerPage": "25",
//!   "entry": [
//!     {
//!       "dc:identifier": "doi:10.1016/...",
//!       "dc:title": "Article Title",
//!       "dc:creator": "Smith J",
//!       "prism:doi": "10.1016/...",
//!       "prism:publicationName": "Nature",
//!       "prism:volume": "56",
//!       "prism:issueIdentifier": "3",
//!       "prism:coverDate": "2024-01-15",
//!       "prism:pageRange": "100-110",
//!       "prism:issn": "0028-0836",
//!       "dc:description": "Abstract text..."
//!     }
//!   ]
//! }
//! ```

use bib_types::convert::{build_article, normalize_doi, parse_author_name};
use bib_types::{Article, ArticleSource, Author, IdKind, Identifier};

use crate::types::{SearchEntry, SearchResponse};

// ---------------------------------------------------------------------------
// SearchResponse → Vec<Article>
// ---------------------------------------------------------------------------

/// Parse an Embase search response into typed [`Article`]s.
///
/// Articles are returned in entry order.
pub fn search_results_to_articles(resp: &SearchResponse) -> Vec<Article> {
    resp.entry.iter().map(search_entry_to_article).collect()
}

/// Parse a single [`SearchEntry`] into an [`Article`].
pub fn search_entry_to_article(entry: &SearchEntry) -> Article {
    // DOI from prism:doi, falling back to dc:identifier if it starts with "doi:"
    let doi = entry.doi.as_deref().filter(|d| !d.is_empty());
    let doi_from_id = entry.identifier.as_deref().and_then(|id| {
        let d = id.strip_prefix("doi:")?.trim();
        if d.is_empty() {
            None
        } else {
            Some(d)
        }
    });

    let mut identifiers: Vec<Identifier> = Vec::new();

    // Prefer the explicit prism:doi; fall back to dc:identifier.
    let doi_value = doi.or(doi_from_id);
    if let Some(d) = doi_value {
        identifiers.push(Identifier::doi(normalize_doi(d)));
    }

    // Embase accession number from dc:identifier (e.g. "EMBASE_123456789")
    if let Some(ref id) = entry.identifier {
        if let Some(embase_id) = id.strip_prefix("EMBASE_") {
            identifiers.push(Identifier::new(IdKind::Embase, embase_id));
        } else if id.starts_with("PMID_") || id.starts_with("MEDLINE_") {
            let pmid = id.split('_').nth(1).unwrap_or("");
            if !pmid.is_empty() {
                identifiers.push(Identifier::pmid(pmid));
            }
        }
    }

    // Authors — Embase provides dc:creator (first author) as a single string.
    // Some responses include a pipe-separated author list in other fields;
    // we parse what we have.
    let authors = parse_authors(entry);

    // Use the DOI (or Embase ID, or title hash) as the internal id.
    let internal_id = identifiers
        .first()
        .map(|i| format!("{}:{}", i.kind.as_str(), i.value))
        .unwrap_or_else(|| {
            // Fallback: use a truncated title.
            format!("embase:{}", &entry.title[..entry.title.len().min(40)])
        });

    let mut article = build_article(
        internal_id,
        entry.title.trim(),
        identifiers,
        authors,
        ArticleSource::Embase,
    );

    // Journal
    if let Some(ref name) = entry.publication_name {
        if !name.is_empty() {
            article.journal = Some(name.clone());
        }
    }

    // Volume / issue / pages
    article.volume = entry.volume.clone().filter(|s| !s.is_empty());
    article.issue = entry.issue_identifier.clone().filter(|s| !s.is_empty());
    article.pages = entry.page_range.clone().filter(|s| !s.is_empty());

    // ISSN / EISSN
    article.issn = entry.issn.clone().filter(|s| !s.is_empty());
    article.essn = entry.eissn.clone().filter(|s| !s.is_empty());

    // Date from prism:coverDate (e.g. "2024-01-15" or "2024")
    if let Some(ref date) = entry.cover_date {
        if !date.is_empty() {
            let (year, month) = parse_cover_date(date);
            article.year = year;
            article.month = month;
        }
    }

    // Abstract
    if let Some(ref desc) = entry.description {
        if !desc.is_empty() {
            article.abstract_text = Some(strip_html_tags(desc));
        }
    }

    // Publication type
    if let Some(ref at) = entry.aggregation_type {
        if !at.is_empty() {
            article.pub_types.push(at.clone());
        }
    }

    article
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Parse authors from dc:creator (first author).
///
/// Embase search results only provide the first author via `dc:creator`.
/// Full author lists require a retrieval API call.
fn parse_authors(entry: &SearchEntry) -> Vec<Author> {
    let mut authors = Vec::new();
    if let Some(ref creator) = entry.creator {
        if !creator.is_empty() {
            authors.push(parse_author_name(creator));
        }
    }
    authors
}

/// Parse a cover date string (e.g. `"2024-01-15"`, `"2024-03"`, `"2024"`)
/// into `(year, month)`.
fn parse_cover_date(s: &str) -> (Option<u16>, Option<u8>) {
    let trimmed = s.trim();
    // ISO format: YYYY-MM-DD or YYYY-MM
    let parts: Vec<&str> = trimmed.split('-').collect();
    let year = parts.first().and_then(|p| p.parse::<u16>().ok());
    let month = parts.get(1).and_then(|p| p.parse::<u8>().ok()).filter(|m| *m >= 1 && *m <= 12);
    (year, month)
}

/// Roughly strip common HTML tags from a string.
fn strip_html_tags(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_tag = false;
    for ch in s.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(ch),
            _ => {}
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_entry_to_article_full() {
        let entry = SearchEntry {
            identifier: Some("doi:10.1016/test.embase.2024.123".into()),
            title: "A Great Paper".into(),
            creator: Some("Smith JA".into()),
            doi: Some("10.1016/test.embase.2024.123".into()),
            publication_name: Some("Nature".into()),
            volume: Some("56".into()),
            issue_identifier: Some("3".into()),
            cover_date: Some("2024-01-15".into()),
            page_range: Some("100-110".into()),
            issn: Some("0028-0836".into()),
            description: Some("This is the <b>abstract</b>.".into()),
            aggregation_type: Some("Journal".into()),
            ..Default::default()
        };

        let article = search_entry_to_article(&entry);
        assert_eq!(article.title, "A Great Paper");
        assert_eq!(
            article.doi(),
            Some("10.1016/test.embase.2024.123")
        );
        assert_eq!(article.authors.len(), 1);
        assert_eq!(article.authors[0].last_name, "Smith");
        assert_eq!(article.journal.as_deref(), Some("Nature"));
        assert_eq!(article.volume.as_deref(), Some("56"));
        assert_eq!(article.issue.as_deref(), Some("3"));
        assert_eq!(article.pages.as_deref(), Some("100-110"));
        assert_eq!(article.year, Some(2024));
        assert_eq!(article.month, Some(1));
        assert_eq!(article.issn.as_deref(), Some("0028-0836"));
        assert_eq!(
            article.abstract_text.as_deref(),
            Some("This is the abstract.")
        );
        assert_eq!(article.source, ArticleSource::Embase);
    }

    #[test]
    fn search_entry_minimal() {
        let entry = SearchEntry {
            title: "Minimal paper".into(),
            ..Default::default()
        };
        let article = search_entry_to_article(&entry);
        assert_eq!(article.title, "Minimal paper");
        assert!(article.identifiers.is_empty());
        assert!(article.authors.is_empty());
        assert!(article.doi().is_none());
    }

    #[test]
    fn search_entry_doi_from_identifier() {
        let entry = SearchEntry {
            identifier: Some("doi:10.2000/fromid".into()),
            title: "Test".into(),
            ..Default::default()
        };
        let article = search_entry_to_article(&entry);
        assert_eq!(article.doi(), Some("10.2000/fromid"));
    }

    #[test]
    fn search_entry_embase_accession() {
        let entry = SearchEntry {
            identifier: Some("EMBASE_62849213".into()),
            title: "Test".into(),
            ..Default::default()
        };
        let article = search_entry_to_article(&entry);
        assert_eq!(
            article.identifier(IdKind::Embase),
            Some("62849213")
        );
    }

    #[test]
    fn search_entry_pmid_from_identifier() {
        let entry = SearchEntry {
            identifier: Some("PMID_12345".into()),
            title: "Test".into(),
            ..Default::default()
        };
        let article = search_entry_to_article(&entry);
        assert_eq!(article.pmid(), Some("12345"));
    }

    #[test]
    fn search_response_to_articles() {
        let resp = SearchResponse {
            total_results: "2".into(),
            start_index: "1".into(),
            items_per_page: "25".into(),
            entry: vec![
                SearchEntry {
                    title: "First".into(),
                    doi: Some("10.1/a".into()),
                    ..Default::default()
                },
                SearchEntry {
                    title: "Second".into(),
                    doi: Some("10.2/b".into()),
                    ..Default::default()
                },
            ],
            error: None,
        };
        let articles = search_results_to_articles(&resp);
        assert_eq!(articles.len(), 2);
        assert_eq!(articles[0].title, "First");
        assert_eq!(articles[1].title, "Second");
    }

    #[test]
    fn parse_cover_date_variants() {
        assert_eq!(parse_cover_date("2024-01-15"), (Some(2024), Some(1)));
        assert_eq!(parse_cover_date("2024-03"), (Some(2024), Some(3)));
        assert_eq!(parse_cover_date("2024"), (Some(2024), None));
        assert_eq!(parse_cover_date("2024-13"), (Some(2024), None)); // invalid month
        assert_eq!(parse_cover_date(""), (None, None));
    }
}
