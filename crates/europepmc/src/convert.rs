//! Conversion layer: parse Europe PMC JSON responses into typed
//! [`bib_types::Article`] records.
//!
//! ## Search response → Article
//!
//! The Europe PMC search API returns a `resultList.result` array where each
//! result has varying fields depending on the `resultType` parameter
//! (`lite` vs `core`). This module handles both.
//!
//! When `resultType=core`, the result includes:
//! - `authorList.author[]` — structured author list with first/last name,
//!   initials, ORCID, affiliations.
//! - `journalInfo` — detailed journal metadata (ISSN, ESSN, volume, issue).
//! - `abstractText` — full abstract (may contain HTML).
//! - `meshHeadingList.meshHeading[]` — MeSH descriptors.
//! - `pubTypeList.pubType[]` — publication type list.
//!
//! When `resultType=lite`, only flat fields (`authorString`, `journalTitle`,
//! `pubYear`, etc.) are available.

use bib_types::convert::{build_article, normalize_doi};
use bib_types::{Article, ArticleSource, Author, IdKind, Identifier};

use crate::types::{SearchResponse, SearchResult};

// ---------------------------------------------------------------------------
// SearchResponse → Vec<Article>
// ---------------------------------------------------------------------------

/// Parse a Europe PMC search response into typed [`Article`]s.
///
/// Articles are returned in result order.
pub fn results_to_articles(resp: &SearchResponse) -> Vec<Article> {
    resp.result_list
        .results
        .iter()
        .map(result_to_article)
        .collect()
}

/// Re-export with a clearer name for a single result.
pub fn result_to_article(result: &SearchResult) -> Article {
    let mut identifiers: Vec<Identifier> = Vec::new();

    // PMID (the `id` field for MED source is the PMID; we also check `pmid`).
    let pmid = result.pmid.as_deref().or(if result.source == "MED" {
        Some(result.id.as_str())
    } else {
        None
    });
    if let Some(p) = pmid.filter(|s| !s.is_empty()) {
        identifiers.push(Identifier::pmid(p));
    }

    // DOI.
    if let Some(ref d) = result.doi {
        let nd = normalize_doi(d);
        if !nd.is_empty() {
            identifiers.push(Identifier::doi(nd));
        }
    }

    // PMC ID: if source is PMC, the id is a PMC identifier.
    if result.source == "PMC" && !result.id.is_empty() {
        identifiers.push(Identifier::new(IdKind::Pmc, &result.id));
    }

    // Authors: prefer structured author list (core), fall back to authorString.
    let authors = if let Some(ref al) = result.author_list {
        al.authors.iter().map(author_detail_to_author).collect()
    } else if let Some(ref astr) = result.author_string {
        parse_author_string(astr)
    } else {
        Vec::new()
    };

    // Internal id: use the first identifier or fall back to source:id.
    let internal_id = identifiers
        .first()
        .map(|i| format!("{}:{}", i.kind.as_str(), i.value))
        .unwrap_or_else(|| format!("{}:{}", result.source, result.id));

    let mut article = build_article(
        internal_id,
        result.title.trim(),
        identifiers,
        authors,
        ArticleSource::EuropePmc,
    );

    // Journal — prefer journalInfo.journal.title (core) over journalTitle (lite).
    if let Some(ref ji) = result.journal_info {
        if let Some(ref j) = ji.journal {
            if let Some(ref title) = j.title {
                if !title.is_empty() {
                    article.journal = Some(title.clone());
                }
            }
            article.issn = j.issn.clone().filter(|s| !s.is_empty());
            article.essn = j.essn.clone().filter(|s| !s.is_empty());
        }
        article.volume = ji.volume.clone().filter(|s| !s.is_empty());
        article.issue = ji.issue.clone().filter(|s| !s.is_empty());
        if let Some(y) = ji.year_of_publication {
            article.year = Some(y);
        }
        if let Some(m) = ji.month_of_publication {
            article.month = Some(m);
        }
    } else {
        // Lite fields.
        article.journal = result.journal_title.clone().filter(|s| !s.is_empty());
        article.volume = result.journal_volume.clone().filter(|s| !s.is_empty());
        article.issue = result.issue.clone().filter(|s| !s.is_empty());
        if let Some(ref y) = result.pub_year {
            if let Ok(yr) = y.parse::<u16>() {
                article.year = Some(yr);
            }
        }
    }

    // Pages.
    article.pages = result.page_info.clone().filter(|s| !s.is_empty());

    // Abstract (may contain HTML).
    if let Some(ref abs) = result.abstract_text {
        if !abs.is_empty() {
            article.abstract_text = Some(strip_html_tags(abs));
        }
    }

    // Language.
    article.language = result.language.clone().filter(|s| !s.is_empty());

    // Publication types.
    if let Some(ref ptl) = result.pub_type_list {
        for pt in &ptl.types {
            article.pub_types.push(pt.clone());
        }
    } else if let Some(ref pt) = result.pub_type {
        // lite: semicolon-separated string like "review; journal article".
        for p in pt.split(';') {
            let trimmed = p.trim();
            if !trimmed.is_empty() {
                article.pub_types.push(trimmed.to_owned());
            }
        }
    }

    // MeSH headings → keywords.
    if let Some(ref mhl) = result.mesh_heading_list {
        for mh in &mhl.headings {
            article.keywords.push(mh.descriptor_name.clone());
        }
    }

    // First affiliation.
    if let Some(ref aff) = result.affiliation {
        if !aff.is_empty() && !article.authors.is_empty() {
            article.authors[0].affiliation = Some(strip_html_tags(aff));
        }
    }

    article
}

// ---------------------------------------------------------------------------
// Author helpers
// ---------------------------------------------------------------------------

/// Convert a structured [`AuthorDetail`] to a bib_types [`Author`].
fn author_detail_to_author(a: &crate::types::AuthorDetail) -> Author {
    let orcid = a
        .author_id
        .as_ref()
        .filter(|id| id.id_type.eq_ignore_ascii_case("ORCID"))
        .map(|id| id.value.clone());

    let affiliation = a
        .affiliation_details
        .as_ref()
        .and_then(|d| d.affiliations.first())
        .map(|e| strip_html_tags(&e.affiliation))
        .filter(|s| !s.is_empty());

    Author {
        last_name: a.last_name.clone().unwrap_or_else(|| a.full_name.clone()),
        fore_name: a.first_name.clone(),
        initials: a.initials.clone(),
        affiliation,
        orcid,
        corresponding: false,
    }
}

/// Parse an `authorString` like `"Fang Y, Zhang T, Lin J, Ye Q."` into
/// [`Author`]s.
fn parse_author_string(s: &str) -> Vec<Author> {
    let trimmed = s.trim_end_matches('.');
    trimmed
        .split(", ")
        .filter(|p| !p.is_empty())
        .map(|p| {
            let p = p.trim();
            match p.rsplit_once(' ') {
                Some((given, family)) if !family.is_empty() => Author {
                    last_name: family.to_owned(),
                    fore_name: Some(given.to_owned()),
                    initials: Some(given.chars().filter(|c| c.is_ascii_uppercase()).collect()),
                    affiliation: None,
                    orcid: None,
                    corresponding: false,
                },
                _ => Author {
                    last_name: p.to_owned(),
                    fore_name: None,
                    initials: None,
                    affiliation: None,
                    orcid: None,
                    corresponding: false,
                },
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// HTML helpers
// ---------------------------------------------------------------------------

/// Roughly strip HTML tags from a string.
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
    use crate::types::*;

    fn lite_result() -> SearchResult {
        SearchResult {
            id: "42541598".into(),
            source: "MED".into(),
            pmid: Some("42541598".into()),
            doi: Some("10.1007/s11033-026-12527-x".into()),
            title: "Mutant p53 and cancer.".into(),
            author_string: Some("Fang Y, Zhang T, Ye Q.".into()),
            journal_title: Some("Mol Biol Rep".into()),
            pub_year: Some("2026".into()),
            journal_volume: Some("53".into()),
            issue: Some("1".into()),
            page_info: Some("1320".into()),
            journal_issn: Some("0301-4851".into()),
            pub_type: Some("journal article".into()),
            cited_by_count: Some(0),
            first_publication_date: Some("2026-08-01".into()),
            ..Default::default()
        }
    }

    #[test]
    fn lite_result_to_article() {
        let result = lite_result();
        let article = result_to_article(&result);
        assert_eq!(article.title, "Mutant p53 and cancer.");
        assert_eq!(article.pmid(), Some("42541598"));
        assert_eq!(article.doi(), Some("10.1007/s11033-026-12527-x"));
        assert_eq!(article.authors.len(), 3);
        assert_eq!(article.authors[0].last_name, "Y");
        assert_eq!(article.authors[1].last_name, "T");
        assert_eq!(article.authors[2].last_name, "Q");
        assert_eq!(article.journal.as_deref(), Some("Mol Biol Rep"));
        assert_eq!(article.year, Some(2026));
        assert_eq!(article.volume.as_deref(), Some("53"));
        assert_eq!(article.issue.as_deref(), Some("1"));
        assert_eq!(article.pages.as_deref(), Some("1320"));
        assert_eq!(article.source, ArticleSource::EuropePmc);
    }

    #[test]
    fn core_result_with_authors_and_mesh() {
        let result = SearchResult {
            id: "123".into(),
            source: "MED".into(),
            pmid: Some("123".into()),
            title: "Test paper".into(),
            author_list: Some(AuthorListWrapper {
                authors: vec![
                    AuthorDetail {
                        full_name: "Smith J".into(),
                        first_name: Some("John".into()),
                        last_name: Some("Smith".into()),
                        initials: Some("J".into()),
                        author_id: Some(AuthorId {
                            id_type: "ORCID".into(),
                            value: "0000-0001-2345-6789".into(),
                        }),
                        affiliation_details: Some(AffiliationDetailsList {
                            affiliations: vec![AffiliationEntry {
                                affiliation: "Harvard University".into(),
                            }],
                        }),
                    },
                    AuthorDetail {
                        full_name: "Doe K".into(),
                        first_name: Some("Karen".into()),
                        last_name: Some("Doe".into()),
                        initials: Some("K".into()),
                        ..Default::default()
                    },
                ],
            }),
            journal_info: Some(JournalInfo {
                volume: Some("10".into()),
                issue: Some("2".into()),
                year_of_publication: Some(2024),
                month_of_publication: Some(3),
                journal: Some(Journal {
                    title: Some("Nature".into()),
                    issn: Some("0028-0836".into()),
                    essn: Some("1476-4687".into()),
                    ..Default::default()
                }),
                ..Default::default()
            }),
            abstract_text: Some("This is a <b>great</b> abstract.".into()),
            pub_type_list: Some(PubTypeList {
                types: vec!["Journal Article".into(), "Review".into()],
            }),
            mesh_heading_list: Some(MeshHeadingList {
                headings: vec![MeshHeading {
                    descriptor_name: "Neoplasms".into(),
                    major_topic: "Y".into(),
                }],
            }),
            ..Default::default()
        };

        let article = result_to_article(&result);
        assert_eq!(article.authors.len(), 2);
        assert_eq!(article.authors[0].last_name, "Smith");
        assert_eq!(article.authors[0].fore_name.as_deref(), Some("John"));
        assert_eq!(
            article.authors[0].orcid.as_deref(),
            Some("0000-0001-2345-6789")
        );
        assert_eq!(
            article.authors[0].affiliation.as_deref(),
            Some("Harvard University")
        );
        assert_eq!(article.journal.as_deref(), Some("Nature"));
        assert_eq!(article.issn.as_deref(), Some("0028-0836"));
        assert_eq!(article.essn.as_deref(), Some("1476-4687"));
        assert_eq!(article.year, Some(2024));
        assert_eq!(article.month, Some(3));
        assert_eq!(
            article.abstract_text.as_deref(),
            Some("This is a great abstract.")
        );
        assert!(article.pub_types.contains(&"Journal Article".to_owned()));
        assert!(article.pub_types.contains(&"Review".to_owned()));
        assert!(article.keywords.contains(&"Neoplasms".to_owned()));
    }

    #[test]
    fn minimal_result() {
        let result = SearchResult {
            id: "1".into(),
            source: "MED".into(),
            title: "Bare title".into(),
            ..Default::default()
        };
        let article = result_to_article(&result);
        assert_eq!(article.title, "Bare title");
        assert_eq!(article.pmid(), Some("1"));
        assert!(article.authors.is_empty());
    }

    #[test]
    fn parse_author_string_basic() {
        let authors = parse_author_string("Smith J, Doe K.");
        assert_eq!(authors.len(), 2);
        assert_eq!(authors[0].last_name, "J");
        assert_eq!(authors[1].last_name, "K");
    }

    #[test]
    fn parse_author_string_single() {
        let authors = parse_author_string("Galileo");
        assert_eq!(authors.len(), 1);
        assert_eq!(authors[0].last_name, "Galileo");
    }

    #[test]
    fn parse_author_string_empty() {
        let authors = parse_author_string("");
        assert!(authors.is_empty());
    }
}
