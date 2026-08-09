//! Conversion layer: map Semantic Scholar [`Paper`] into the shared
//! [`bib_types::Article`] model.

use bib_types::convert::{build_article, normalize_doi};
use bib_types::{Article, ArticleSource, Author, IdKind, Identifier};

use crate::types::Paper;

// ---------------------------------------------------------------------------
// Paper → Article
// ---------------------------------------------------------------------------

/// Convert a Semantic Scholar [`Paper`] into a typed [`Article`].
pub fn paper_to_article(paper: &Paper) -> Article {
    let mut identifiers: Vec<Identifier> = Vec::new();

    // DOI
    if let Some(ref ext) = paper.external_ids {
        if let Some(ref doi) = ext.doi {
            let nd = normalize_doi(doi);
            if !nd.is_empty() {
                identifiers.push(Identifier::doi(nd));
            }
        }
        if let Some(ref pmid) = ext.pubmed {
            if !pmid.is_empty() {
                identifiers.push(Identifier::pmid(pmid));
            }
        }
        if let Some(ref arxiv) = ext.arxiv {
            if !arxiv.is_empty() {
                identifiers.push(Identifier::new(IdKind::Arxiv, arxiv));
            }
        }
        if let Some(ref pmc) = ext.pubmed_central {
            if !pmc.is_empty() {
                identifiers.push(Identifier::new(IdKind::Pmc, pmc));
            }
        }
    }

    // Use S2 paperId as internal ID
    let internal_id = if identifiers.is_empty() {
        format!("S2:{}", paper.paper_id)
    } else {
        identifiers
            .first()
            .map(|i| format!("{}:{}", i.kind.as_str(), i.value))
            .unwrap_or_else(|| format!("S2:{}", paper.paper_id))
    };

    // Authors
    let authors: Vec<Author> = paper
        .authors
        .iter()
        .map(|a| {
            let (last_name, fore_name) = split_name(a.name.as_deref().unwrap_or(""));
            Author {
                last_name,
                fore_name,
                initials: None,
                affiliation: None,
                orcid: None,
                corresponding: false,
            }
        })
        .collect();

    let title = paper.title.as_deref().unwrap_or("").trim();
    let mut article = build_article(
        internal_id,
        title,
        identifiers,
        authors,
        ArticleSource::SemanticScholar,
    );

    // Year
    if let Some(y) = paper.year {
        if y > 0 {
            article.year = Some(y as u16);
        }
    }

    // Publication date → month
    if let Some(ref pd) = paper.publication_date {
        if let Some(m) = pd.get(5..7).and_then(|s| s.parse::<u8>().ok()) {
            article.month = Some(m);
        }
    }

    // Venue / journal
    article.journal = paper.venue.clone().filter(|s| !s.is_empty());
    if let Some(ref j) = paper.journal {
        if let Some(ref name) = j.name {
            if !name.is_empty() {
                article.journal = Some(name.clone());
            }
        }
        article.volume = j.volume.clone().filter(|s| !s.is_empty());
        article.pages = j.pages.clone().filter(|s| !s.is_empty());
    }

    // Abstract
    article.abstract_text = paper.abstract_text.clone().filter(|s| !s.is_empty());

    // Publication types
    if let Some(ref pts) = paper.publication_types {
        for pt in pts {
            article.pub_types.push(pt.clone());
        }
    }

    // Fields of study → keywords
    if let Some(ref fos) = paper.fields_of_study {
        for f in fos {
            article.keywords.push(f.clone());
        }
    }

    // TLDR → notes (store in keywords with prefix for retrieval)
    if let Some(ref tldr) = paper.tldr {
        if let Some(ref text) = tldr.text {
            if !text.is_empty() {
                article.keywords.push(format!("TLDR: {text}"));
            }
        }
    }

    article
}

/// Split a full name into (last_name, fore_name).
fn split_name(full: &str) -> (String, Option<String>) {
    let trimmed = full.trim();
    if trimmed.is_empty() {
        return (String::new(), None);
    }
    match trimmed.rsplit_once(' ') {
        Some((given, family)) if !family.is_empty() => {
            (family.to_string(), Some(given.to_string()))
        }
        _ => (trimmed.to_string(), None),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::*;

    #[test]
    fn paper_to_article_basic() {
        let paper = Paper {
            paper_id: "abc123".into(),
            title: Some("Test Paper".into()),
            year: Some(2024),
            venue: Some("Nature".into()),
            authors: vec![
                PaperAuthor { author_id: Some("1".into()), name: Some("Alice Smith".into()) },
                PaperAuthor { author_id: Some("2".into()), name: Some("Bob Jones".into()) },
            ],
            external_ids: Some(ExternalIds {
                doi: Some("10.1038/nbt.1234".into()),
                pubmed: Some("123456".into()),
                ..Default::default()
            }),
            ..Default::default()
        };
        let article = paper_to_article(&paper);
        assert_eq!(article.title, "Test Paper");
        assert_eq!(article.source, ArticleSource::SemanticScholar);
        assert_eq!(article.doi(), Some("10.1038/nbt.1234"));
        assert_eq!(article.pmid(), Some("123456"));
        assert_eq!(article.year, Some(2024));
        assert_eq!(article.journal.as_deref(), Some("Nature"));
        assert_eq!(article.authors.len(), 2);
        assert_eq!(article.authors[0].last_name, "Smith");
        assert_eq!(article.authors[0].fore_name.as_deref(), Some("Alice"));
    }

    #[test]
    fn paper_minimal() {
        let paper = Paper {
            paper_id: "xyz".into(),
            title: Some("Bare".into()),
            ..Default::default()
        };
        let article = paper_to_article(&paper);
        assert_eq!(article.title, "Bare");
        assert_eq!(article.authors.len(), 0);
    }

    #[test]
    fn split_name_single() {
        let (last, fore) = split_name("Galileo");
        assert_eq!(last, "Galileo");
        assert_eq!(fore, None);
    }

    #[test]
    fn split_name_multi() {
        let (last, fore) = split_name("John von Neumann");
        assert_eq!(last, "Neumann");
        assert_eq!(fore.as_deref(), Some("John von"));
    }
}
