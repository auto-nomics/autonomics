//! Conversion layer: map Semantic Scholar [`Paper`] into the shared
//! [`bib_types::Article`] model.
//!
//! ## Identifier strategy
//!
//! Every external ID from the S2 `externalIds` object is normalised and
//! pushed into [`Article::identifiers`], so the shared dedup pipeline can
//! match on any key (DOI, PMID, ArXiv, PMC). The S2 `paperId` is always
//! added as [`IdKind::S2`] to preserve the round-trip link back to
//! Semantic Scholar.

use bib_types::convert::{build_article, normalize_doi};
use bib_types::{Article, ArticleSource, Author, IdKind, Identifier};

use crate::types::Paper;

// ---------------------------------------------------------------------------
// Paper → Article
// ---------------------------------------------------------------------------

/// Convert a Semantic Scholar [`Paper`] into a typed [`Article`].
pub fn paper_to_article(paper: &Paper) -> Article {
    let mut identifiers: Vec<Identifier> = Vec::new();

    // External IDs — DOI first (highest dedup priority).
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

    // Always add the S2 paperId as an identifier so it survives dedup.
    if !paper.paper_id.is_empty() {
        identifiers.push(Identifier::new(IdKind::S2, &paper.paper_id));
    }

    // Internal id: prefer DOI, fall back to S2 ID.
    let internal_id = identifiers
        .first()
        .map(|i| format!("{}:{}", i.kind.as_str(), i.value))
        .unwrap_or_else(|| paper.paper_id.clone());

    // Authors — S2 uses full display names ("Alice Smith"), so split at the
    // last space to get (fore_name, last_name).
    let authors: Vec<Author> = paper
        .authors
        .iter()
        .filter_map(|a| {
            let name = a.name.as_deref()?.trim();
            if name.is_empty() {
                return None;
            }
            Some(split_name(name))
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

    // Publication date (YYYY-MM-DD) → month
    if let Some(ref pd) = paper.publication_date {
        if let Some(m) = pd.get(5..7).and_then(|s| s.parse::<u8>().ok()) {
            article.month = Some(m);
        }
    }

    // Venue / journal — prefer journal.name, fall back to venue.
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

    // NOTE: TLDR is S2-specific metadata with no home in the shared Article
    // model. It is intentionally NOT stuffed into keywords (which would
    // pollute dedup/search). Callers who need TLDR should use the raw
    // `Paper` directly via the SDK client.

    article
}

/// Convert a batch of Semantic Scholar [`Paper`]s into typed [`Article`]s.
///
/// Convenience wrapper that maps [`paper_to_article`] over a slice.
pub fn papers_to_articles(papers: &[Paper]) -> Vec<Article> {
    papers.iter().map(paper_to_article).collect()
}

// ---------------------------------------------------------------------------
// Name splitting
// ---------------------------------------------------------------------------

/// Split a full display name into an [`Author`].
///
/// S2 author names are full display names (e.g. `"Alice Smith"`,
/// `"Oren Etzioni"`). We split at the last space to obtain `(fore_name,
/// last_name)`. This differs from [`bib_types::convert::parse_author_name`]
/// which assumes the PubMed `"LastName Initials"` convention.
fn split_name(full: &str) -> Author {
    match full.rsplit_once(' ') {
        Some((given, family)) if !family.is_empty() => Author {
            last_name: family.to_string(),
            fore_name: Some(given.to_string()),
            initials: None,
            affiliation: None,
            orcid: None,
            corresponding: false,
        },
        _ => Author {
            last_name: full.to_string(),
            fore_name: None,
            initials: None,
            affiliation: None,
            orcid: None,
            corresponding: false,
        },
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
    fn s2_paperid_stored_as_identifier() {
        let paper = Paper {
            paper_id: "abc123sha".into(),
            title: Some("Test".into()),
            ..Default::default()
        };
        let article = paper_to_article(&paper);
        assert_eq!(
            article.identifier(IdKind::S2),
            Some("abc123sha"),
            "S2 paperId must be in identifiers for round-trip dedup"
        );
    }

    #[test]
    fn s2_paperid_always_present_even_with_doi() {
        let paper = Paper {
            paper_id: "shaXYZ".into(),
            title: Some("Test".into()),
            external_ids: Some(ExternalIds {
                doi: Some("10.1000/test".into()),
                ..Default::default()
            }),
            ..Default::default()
        };
        let article = paper_to_article(&paper);
        // Both DOI and S2 should be present.
        assert_eq!(article.doi(), Some("10.1000/test"));
        assert_eq!(article.identifier(IdKind::S2), Some("shaXYZ"));
    }

    #[test]
    fn tldr_not_in_keywords() {
        let paper = Paper {
            paper_id: "xyz".into(),
            title: Some("Test".into()),
            tldr: Some(Tldr {
                model: Some("tldr@v2".into()),
                text: Some("A short summary.".into()),
            }),
            ..Default::default()
        };
        let article = paper_to_article(&paper);
        assert!(
            !article.keywords.iter().any(|k| k.contains("TLDR")),
            "TLDR must not pollute keywords"
        );
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
    fn batch_conversion() {
        let papers = vec![
            Paper { paper_id: "1".into(), title: Some("First".into()), ..Default::default() },
            Paper { paper_id: "2".into(), title: Some("Second".into()), ..Default::default() },
        ];
        let articles = papers_to_articles(&papers);
        assert_eq!(articles.len(), 2);
        assert_eq!(articles[0].title, "First");
        assert_eq!(articles[1].title, "Second");
    }

    #[test]
    fn split_name_single() {
        let a = split_name("Galileo");
        assert_eq!(a.last_name, "Galileo");
        assert!(a.fore_name.is_none());
    }

    #[test]
    fn split_name_multi() {
        let a = split_name("John von Neumann");
        assert_eq!(a.last_name, "Neumann");
        assert_eq!(a.fore_name.as_deref(), Some("John von"));
    }

    #[test]
    fn split_name_empty_filtered() {
        let paper = Paper {
            paper_id: "x".into(),
            title: Some("T".into()),
            authors: vec![
                PaperAuthor { author_id: None, name: Some("Real Person".into()) },
                PaperAuthor { author_id: None, name: Some("".into()) },
                PaperAuthor { author_id: None, name: None },
            ],
            ..Default::default()
        };
        let article = paper_to_article(&paper);
        assert_eq!(article.authors.len(), 1);
        assert_eq!(article.authors[0].last_name, "Person");
    }
}
