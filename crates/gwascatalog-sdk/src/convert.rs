//! Conversion layer: parse GWAS Catalog publication metadata into typed
//! [`bib_types::Article`] records.
//!
//! The GWAS Catalog embeds publication info inside study records
//! ([`PublicationInfo`]) and Solr search docs ([`SearchDoc`]). This module
//! normalises both into the canonical [`bib_types::Article`] type so that
//! literature from PubMed, GWAS Catalog, and other sources share a single
//! representation.

use bib_types::convert::{build_article, parse_author_name};
use bib_types::{Article, ArticleSource, IdKind, Identifier};

use crate::rest::PublicationInfo;
use crate::search::SearchDoc;

// ---------------------------------------------------------------------------
// PublicationInfo (REST API) → Article
// ---------------------------------------------------------------------------

/// Convert a GWAS Catalog REST [`PublicationInfo`] into a partial
/// [`bib_types::Article`].
///
/// The GWAS Catalog provides only the first author (as a single string) and
/// a publication date string — the resulting [`Article`] may have fewer
/// fields than one parsed from PubMed. Use the PMID to cross-fill from
/// ESummary if complete metadata is needed.
pub fn publication_info_to_article(pi: &PublicationInfo) -> Article {
    let pmid = pi.pubmed_id.as_deref().unwrap_or("");

    // Identifiers — PMID + DOI (GWAS Catalog doesn't return DOI in PublicationInfo)
    let mut identifiers: Vec<Identifier> = Vec::new();
    if !pmid.is_empty() {
        identifiers.push(Identifier::pmid(pmid));
    }

    // Author — single string from the REST API
    let authors = pi
        .author
        .as_ref()
        .and_then(|a| a.fullname.as_deref())
        .map(|name| vec![parse_author_name(name)])
        .unwrap_or_default();

    let title = pi.title.clone().unwrap_or_default();
    let mut article = build_article(
        format!("gwas-pub-{pmid}"),
        title,
        identifiers,
        authors,
        ArticleSource::GwasCatalog,
    );

    // Journal / publication venue
    if let Some(ref pub_name) = pi.publication {
        article.journal = Some(pub_name.clone());
    }

    // Date — GWAS Catalog uses "2024-01-15" or "2024-03" format
    if let Some(ref date) = pi.publication_date {
        let (year, month) = parse_gwas_date(date);
        article.year = year;
        article.month = month;
    }

    // ORCID if present
    if let Some(ref author) = pi.author {
        if let Some(ref orcid) = author.orcid {
            if let Some(a) = article.authors.first_mut() {
                a.orcid = Some(strip_orcid_url(orcid));
            }
        }
    }

    article
}

// ---------------------------------------------------------------------------
// SearchDoc (Solr) → Article
// ---------------------------------------------------------------------------

/// Convert a GWAS Catalog Solr [`SearchDoc`] of resource type
/// `"publication"` into a partial [`bib_types::Article`].
pub fn search_doc_to_article(doc: &SearchDoc) -> Article {
    let pmid = doc.pmid.as_deref().unwrap_or("");

    let mut identifiers: Vec<Identifier> = Vec::new();
    if !pmid.is_empty() {
        identifiers.push(Identifier::pmid(pmid));
    }

    // Solr returns authors as an array of display-name strings
    let authors = doc
        .author
        .iter()
        .map(|name| parse_author_name(name))
        .collect::<Vec<_>>();

    let title = doc.title.clone().unwrap_or_default();
    let mut article = build_article(
        format!("gwas-solr-{pmid}"),
        title,
        identifiers,
        authors,
        ArticleSource::GwasCatalog,
    );

    if let Some(ref journal) = doc.journal {
        article.journal = Some(journal.clone());
    }

    if let Some(ref date) = doc.publication_date {
        let (year, month) = parse_gwas_date(date);
        article.year = year;
        article.month = month;
    }

    article
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Parse a GWAS Catalog date string (`"2024-01-15"`, `"2024-03"`,
/// `"2024"`) into `(year, month)`.
fn parse_gwas_date(s: &str) -> (Option<u16>, Option<u8>) {
    let parts: Vec<&str> = s.split('-').collect();
    let year = parts.first().and_then(|y| y.parse::<u16>().ok());
    let month = parts.get(1).and_then(|m| m.parse::<u8>().ok());
    (year, month)
}

/// Strip an ORCID URL prefix (`"https://orcid.org/0000-..."`)
/// to the bare ID.
fn strip_orcid_url(s: &str) -> String {
    s.trim()
        .strip_prefix("https://orcid.org/")
        .or_else(|| s.trim().strip_prefix("http://orcid.org/"))
        .unwrap_or(s.trim())
        .to_owned()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rest::{PublicationAuthor};
    use crate::search::SearchDoc;

    #[test]
    fn publication_info_basic() {
        let pi = PublicationInfo {
            pubmed_id: Some("12345".into()),
            publication_date: Some("2024-03-15".into()),
            publication: Some("Nature Genetics".into()),
            title: Some("A GWAS study".into()),
            author: Some(PublicationAuthor {
                fullname: Some("Smith JA".into()),
                orcid: Some("https://orcid.org/0000-0002-1825-0097".into()),
            }),
        };

        let a = publication_info_to_article(&pi);
        assert_eq!(a.pmid(), Some("12345"));
        assert_eq!(a.title, "A GWAS study");
        assert_eq!(a.journal.as_deref(), Some("Nature Genetics"));
        assert_eq!(a.year, Some(2024));
        assert_eq!(a.month, Some(3));
        assert_eq!(a.authors.len(), 1);
        assert_eq!(a.authors[0].last_name, "Smith");
        assert_eq!(a.authors[0].initials.as_deref(), Some("JA"));
        assert_eq!(
            a.authors[0].orcid.as_deref(),
            Some("0000-0002-1825-0097")
        );
    }

    #[test]
    fn publication_info_no_pmid() {
        let pi = PublicationInfo::default();
        let a = publication_info_to_article(&pi);
        assert!(a.pmid().is_none());
        assert!(a.authors.is_empty());
    }

    #[test]
    fn search_doc_publication() {
        let doc = SearchDoc {
            resourcename: Some("publication".into()),
            pmid: Some("99999".into()),
            title: Some("Solr paper".into()),
            journal: Some("Science".into()),
            publication_date: Some("2023-06".into()),
            author: vec!["Doe J".into(), "Roe R".into()],
            ..Default::default()
        };

        let a = search_doc_to_article(&doc);
        assert_eq!(a.pmid(), Some("99999"));
        assert_eq!(a.title, "Solr paper");
        assert_eq!(a.journal.as_deref(), Some("Science"));
        assert_eq!(a.year, Some(2023));
        assert_eq!(a.month, Some(6));
        assert_eq!(a.authors.len(), 2);
    }

    #[test]
    fn parse_gwas_date_variants() {
        assert_eq!(parse_gwas_date("2024-01-15"), (Some(2024), Some(1)));
        assert_eq!(parse_gwas_date("2024-06"), (Some(2024), Some(6)));
        assert_eq!(parse_gwas_date("2024"), (Some(2024), None));
    }

    #[test]
    fn strip_orcid_url_variants() {
        assert_eq!(
            strip_orcid_url("https://orcid.org/0000-0002-1825-0097"),
            "0000-0002-1825-0097"
        );
        assert_eq!(strip_orcid_url("0000-0002-1825-0097"), "0000-0002-1825-0097");
    }
}
