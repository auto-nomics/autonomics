//! Conversion layer: parse Crossref JSON [`Work`] records into typed
//! [`bib_types::Article`] records.
//!
//! This makes Crossref data interchangeable with records from PubMed, arXiv,
//! Europe PMC, etc.

use bib_types::convert::{build_article, normalize_doi};
use bib_types::{Article, ArticleSource, Author, IdKind, Identifier};

use crate::types::Work;

/// Convert a Crossref [`Work`] into a typed [`Article`].
pub fn work_to_article(work: &Work) -> Article {
    let mut identifiers: Vec<Identifier> = Vec::new();

    // DOI.
    if !work.doi.is_empty() {
        let nd = normalize_doi(&work.doi);
        if !nd.is_empty() {
            identifiers.push(Identifier::doi(nd));
        }
    }

    // ISBN (stored as Other).
    for isbn in &work.isbn {
        identifiers.push(Identifier::new(IdKind::Other, isbn));
    }

    // Authors.
    let authors: Vec<Author> = work
        .author
        .iter()
        .map(|a| {
            let orcid = a
                .orcid
                .as_ref()
                .map(|url| {
                    url.rsplit('/').next().unwrap_or(url).to_string()
                });
            let affiliation = a
                .affiliation
                .first()
                .map(|aff| aff.name.clone())
                .filter(|s| !s.is_empty());
            Author {
                last_name: a.family.clone(),
                fore_name: if a.given.is_empty() {
                    None
                } else {
                    Some(a.given.clone())
                },
                initials: None,
                affiliation,
                orcid: orcid.filter(|s| !s.is_empty()),
                corresponding: a.sequence == "first" || a.sequence == "additional",
            }
        })
        .collect();

    // Internal id: use DOI or fall back to a synthetic id.
    let internal_id = identifiers
        .first()
        .map(|i| format!("{}:{}", i.kind.as_str(), i.value))
        .unwrap_or_else(|| format!("crossref:{}", work.doi));

    let mut article = build_article(
        internal_id,
        work.title_str(),
        identifiers,
        authors,
        ArticleSource::CrossRef,
    );

    // Journal / container title.
    article.journal = work
        .container_title
        .first()
        .cloned()
        .filter(|s| !s.is_empty());

    // ISSN (prefer print ISSN; Crossref doesn't distinguish print/electronic
    // in the ISSN array).
    article.issn = work.issn.first().cloned().filter(|s| !s.is_empty());

    // Volume / issue / pages / article-number.
    article.volume = if work.volume.is_empty() {
        None
    } else {
        Some(work.volume.clone())
    };
    article.issue = if work.issue.is_empty() {
        None
    } else {
        Some(work.issue.clone())
    };
    article.pages = if work.page.is_empty() {
        if work.article_number.is_empty() {
            None
        } else {
            Some(work.article_number.clone())
        }
    } else {
        Some(work.page.clone())
    };

    // Publication date — prefer issued.
    if let Some(ref dp) = work.issued {
        let (y, m, _d) = dp.ymd();
        article.year = y;
        article.month = m;
    } else if let Some(ref dp) = work.published_online {
        let (y, m, _) = dp.ymd();
        article.year = y;
        article.month = m;
    } else if let Some(ref dp) = work.published_print {
        let (y, m, _) = dp.ymd();
        article.year = y;
        article.month = m;
    }

    // Abstract (strip JATS XML tags).
    if let Some(ref abs) = work.abstract_text {
        if !abs.is_empty() {
            article.abstract_text = Some(strip_xml_tags(abs));
        }
    }

    // Type → pub_types.
    if !work.r#type.is_empty() {
        article.pub_types.push(work.r#type.clone());
    }

    // Subject categories → keywords.
    for s in &work.subject {
        article.keywords.push(s.clone());
    }

    // Language.
    article.language = if work.language.is_empty() {
        None
    } else {
        Some(work.language.clone())
    };

    article
}

/// Strip XML/JATS tags from a string.
fn strip_xml_tags(s: &str) -> String {
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
    // Collapse multiple whitespace.
    while out.contains("  ") {
        out = out.replace("  ", " ");
    }
    out.trim().to_string()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{
        Affiliation as CrAffiliation, Author as CrAuthor, DateParts as CrDateParts,
        Work as CrWork,
    };

    fn sample_work() -> CrWork {
        CrWork {
            doi: "10.1037/0003-066x.59.1.29".into(),
            title: vec!["Toward a science of ego depletion".into()],
            container_title: vec!["American Psychologist".into()],
            author: vec![
                CrAuthor {
                    given: "Roy F".into(),
                    family: "Baumeister".into(),
                    sequence: "first".into(),
                    orcid: Some("https://orcid.org/0000-0002-1234-5678".into()),
                    affiliation: vec![CrAffiliation {
                        name: "Florida State University".into(),
                    }],
                },
                CrAuthor {
                    given: "John".into(),
                    family: "Doe".into(),
                    sequence: "additional".into(),
                    orcid: None,
                    affiliation: vec![],
                },
            ],
            issued: Some(CrDateParts {
                date_parts: vec![vec![Some(2004), Some(7)]],
                ..Default::default()
            }),
            abstract_text: Some(
                "<jats:p>Self-regulation involves <jats:italic>ego</jats:italic> depletion.</jats:p>"
                    .into(),
            ),
            volume: "59".into(),
            issue: "1".into(),
            page: "29-36".into(),
            issn: vec!["0003-066X".into()],
            r#type: "journal-article".into(),
            subject: vec!["Psychology".into()],
            is_referenced_by_count: 5000,
            ..Default::default()
        }
    }

    #[test]
    fn convert_basic() {
        let work = sample_work();
        let art = work_to_article(&work);
        assert_eq!(
            art.doi(),
            Some("10.1037/0003-066x.59.1.29")
        );
        assert_eq!(art.title, "Toward a science of ego depletion");
        assert_eq!(art.authors.len(), 2);
        assert_eq!(art.authors[0].last_name, "Baumeister");
        assert_eq!(art.authors[0].fore_name.as_deref(), Some("Roy F"));
        assert_eq!(
            art.authors[0].orcid.as_deref(),
            Some("0000-0002-1234-5678")
        );
        assert_eq!(
            art.authors[0].affiliation.as_deref(),
            Some("Florida State University")
        );
        assert_eq!(art.journal.as_deref(), Some("American Psychologist"));
        assert_eq!(art.year, Some(2004));
        assert_eq!(art.month, Some(7));
        assert_eq!(art.volume.as_deref(), Some("59"));
        assert_eq!(art.issue.as_deref(), Some("1"));
        assert_eq!(art.pages.as_deref(), Some("29-36"));
        assert_eq!(art.issn.as_deref(), Some("0003-066X"));
        assert!(art.pub_types.contains(&"journal-article".to_string()));
        assert!(art.keywords.contains(&"Psychology".to_string()));
    }

    #[test]
    fn convert_strips_jats() {
        let work = sample_work();
        let art = work_to_article(&work);
        let abs = art.abstract_text.as_deref().unwrap();
        assert!(abs.contains("Self-regulation"));
        assert!(abs.contains("ego"));
        assert!(!abs.contains("<jats"));
        assert!(!abs.contains("</jats"));
    }

    #[test]
    fn convert_minimal() {
        let work = CrWork {
            doi: "10.1/xyz".into(),
            title: vec!["Bare".into()],
            ..Default::default()
        };
        let art = work_to_article(&work);
        assert_eq!(art.doi(), Some("10.1/xyz"));
        assert_eq!(art.title, "Bare");
        assert!(art.authors.is_empty());
    }

    #[test]
    fn convert_article_number_as_pages() {
        let work = CrWork {
            doi: "10.1/abc".into(),
            title: vec!["T".into()],
            page: "".into(),
            article_number: "e12345".into(),
            ..Default::default()
        };
        let art = work_to_article(&work);
        assert_eq!(art.pages.as_deref(), Some("e12345"));
    }
}
