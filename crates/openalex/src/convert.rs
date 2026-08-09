//! Conversion layer: parse OpenAlex `Work` records into typed
//! [`bib_types::Article`] records.
//!
//! Every SDK crate in the workspace (`europepmc`, `eutils`, `arxiv`, …)
//! provides a conversion module so that results from heterogeneous sources
//! can be normalised into the single shared [`bib_types::Article`] model.
//! This module does the same for OpenAlex.
//!
//! ## Work → Article
//!
//! Field mapping:
//!
//! | OpenAlex `Work`         | `bib_types::Article` field    |
//! |-------------------------|-------------------------------|
//! | `id` (`W…`)             | `identifiers` (IdKind::OpenAlex) |
//! | `doi`                   | `identifiers` (IdKind::Doi)   |
//! | `ids.pmid`              | `identifiers` (IdKind::Pmid)  |
//! | `ids.pmcid`             | `identifiers` (IdKind::Pmc)   |
//! | `title` / `display_name`| `title`                       |
//! | `publication_year`      | `year`                        |
//! | `publication_date`      | `year`, `month`               |
//! | `authorships`           | `authors`                     |
//! | `primary_location.source` | `journal`, `issn`           |
//! | `biblio`                | `volume`, `issue`, `pages`    |
//! | `abstract_inverted_index` | `abstract_text`             |
//! | `language`              | `language`                    |
//! | `type`                  | `pub_types`                   |
//! | `topics.display_name`   | `keywords`                    |
//! | `keywords.display_name` | `keywords`                    |
//! | `mesh.descriptor_name`  | `keywords`                    |

use bib_types::convert::{build_article, normalize_doi};
use bib_types::{Article, ArticleSource, Author, IdKind, Identifier};

use crate::types::{Authorship, Work};

// ---------------------------------------------------------------------------
// Work → Article
// ---------------------------------------------------------------------------

/// Convert a single [`Work`] into a [`bib_types::Article`].
pub fn work_to_article(work: &Work) -> Article {
    let identifiers = collect_identifiers(work);

    // Authors: OpenAlex gives display_name + optional ORCID.
    let authors: Vec<Author> = work
        .authorships
        .iter()
        .map(authorship_to_author)
        .collect();

    // Internal id: prefer DOI, then OpenAlex id, then fallback.
    let internal_id = identifiers
        .first()
        .map(|i| format!("{}:{}", i.kind.as_str(), i.value))
        .unwrap_or_else(|| work.id.clone());

    let title = work
        .title_or_name()
        .unwrap_or("(untitled)")
        .trim()
        .to_owned();

    let mut article = build_article(
        internal_id,
        title,
        identifiers,
        authors,
        ArticleSource::OpenAlex,
    );

    // Year + month from publication_date (more precise) or publication_year.
    if let Some(ref date) = work.publication_date {
        let (y, m) = parse_iso_date(date);
        if y.is_some() {
            article.year = y;
        } else {
            article.year = work.publication_year;
        }
        article.month = m;
    } else {
        article.year = work.publication_year;
    }

    // Journal + ISSN from primary_location.
    if let Some(ref loc) = work.primary_location {
        if let Some(ref src) = loc.source {
            article.journal = src.display_name.clone().filter(|s| !s.is_empty());
            if let Some(ref issn) = src.issn_l {
                if !issn.is_empty() {
                    article.issn = Some(issn.clone());
                }
            }
        }
    }

    // Biblio: volume, issue, pages.
    if let Some(ref bib) = work.biblio {
        article.volume = bib.volume.clone().filter(|s| !s.is_empty());
        article.issue = bib.issue.clone().filter(|s| !s.is_empty());
        // Combine first_page–last_page into a range string.
        let pages = match (&bib.first_page, &bib.last_page) {
            (Some(fp), Some(lp)) if fp != lp => Some(format!("{fp}-{lp}")),
            (Some(fp), _) => Some(fp.clone()),
            _ => None,
        };
        if let Some(p) = pages.filter(|s| !s.is_empty()) {
            article.pages = Some(p);
        }
    }

    // Abstract reconstructed from inverted index.
    if let Some(abs) = work.abstract_text() {
        if !abs.is_empty() {
            article.abstract_text = Some(abs);
        }
    }

    // Language.
    article.language = work.language.clone().filter(|s| !s.is_empty());

    // Publication type.
    if let Some(ref t) = work.type_ {
        if !t.is_empty() {
            article.pub_types.push(t.clone());
        }
    }

    // Keywords: topics + keywords + mesh.
    for topic in &work.topics {
        if !topic.display_name.is_empty() {
            article.keywords.push(topic.display_name.clone());
        }
    }
    for kw in &work.keywords {
        if !kw.display_name.is_empty() {
            article.keywords.push(kw.display_name.clone());
        }
    }
    for mesh in &work.mesh {
        if let Some(ref name) = mesh.descriptor_name {
            if !name.is_empty() {
                article.keywords.push(name.clone());
            }
        }
    }

    article
}

/// Convert a [`ListResponse<Work>`] into a `Vec<Article>`.
pub fn works_to_articles(resp: &crate::types::ListResponse<Work>) -> Vec<Article> {
    resp.results.iter().map(work_to_article).collect()
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Collect all available identifiers from a [`Work`].
fn collect_identifiers(work: &Work) -> Vec<Identifier> {
    let mut ids = Vec::new();

    // OpenAlex ID — strip the URL prefix to get the bare `W…` form.
    let openalex_id = work
        .id
        .strip_prefix("https://openalex.org/")
        .unwrap_or(&work.id);
    if !openalex_id.is_empty() {
        ids.push(Identifier::new(IdKind::OpenAlex, openalex_id));
    }

    // DOI — normalised.
    if let Some(ref doi) = work.doi {
        let nd = normalize_doi(doi);
        if !nd.is_empty() {
            ids.push(Identifier::doi(nd));
        }
    }

    // PMID — strip URL prefix if present.
    if let Some(ref pmid) = work.ids.pmid {
        let bare = pmid
            .strip_prefix("https://pubmed.ncbi.nlm.nih.gov/")
            .unwrap_or(pmid);
        if !bare.is_empty() {
            ids.push(Identifier::pmid(bare));
        }
    }

    // PMCID.
    if let Some(ref pmcid) = work.ids.pmcid {
        if !pmcid.is_empty() {
            ids.push(Identifier::new(IdKind::Pmc, pmcid));
        }
    }

    ids
}

/// Convert an [`Authorship`] to a bib_types [`Author`].
///
/// OpenAlex gives a single `display_name` (e.g. "Heather A Piwowar") in
/// "FirstName … LastName" order — the opposite of PubMed's
/// "LastName Initials". We split on the **last** space to extract
/// fore_name and last_name, then derive initials from the fore_name.
fn authorship_to_author(a: &Authorship) -> Author {
    let display = a.author.display_name.as_deref().unwrap_or("");
    let mut author = if display.is_empty() {
        Author {
            last_name: String::new(),
            fore_name: None,
            initials: None,
            affiliation: None,
            orcid: None,
            corresponding: false,
        }
    } else {
        parse_display_name(display)
    };

    // ORCID — strip URL prefix.
    if let Some(ref orcid) = a.author.orcid {
        let bare = orcid
            .strip_prefix("https://orcid.org/")
            .unwrap_or(orcid);
        author.orcid = Some(bare.to_owned());
    }

    // First affiliation (if any).
    if let Some(inst) = a.institutions.first() {
        if let Some(ref name) = inst.display_name {
            if !name.is_empty() {
                author.affiliation = Some(name.clone());
            }
        }
    }

    author
}

/// Parse an OpenAlex `display_name` ("First Middle Last") into an [`Author`].
///
/// Splits on the **last** space: everything before is `fore_name`, the last
/// token is `last_name`. Initials are derived from uppercase first letters of
/// `fore_name` tokens.
fn parse_display_name(name: &str) -> Author {
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

    match trimmed.rsplit_once(' ') {
        Some((given, last)) if !last.is_empty() && !given.is_empty() => {
            let initials: String = given
                .split_whitespace()
                .filter_map(|t| t.chars().next())
                .filter(|c| c.is_ascii_uppercase())
                .collect();
            Author {
                last_name: last.to_owned(),
                fore_name: Some(given.to_owned()),
                initials: if initials.is_empty() { None } else { Some(initials) },
                affiliation: None,
                orcid: None,
                corresponding: false,
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

/// Parse an ISO 8601 date (`"2024-03-15"`) into `(year, month)`.
fn parse_iso_date(s: &str) -> (Option<u16>, Option<u8>) {
    let parts: Vec<&str> = s.split('-').collect();
    let year = parts.first().and_then(|p| p.parse::<u16>().ok());
    let month = parts.get(1).and_then(|p| p.parse::<u8>().ok());
    (year, month)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::*;

    fn sample_work() -> Work {
        Work {
            id: "https://openalex.org/W2741809807".into(),
            doi: Some("https://doi.org/10.7717/peerj.4375".into()),
            display_name: Some("The state of OA".into()),
            publication_year: Some(2018),
            publication_date: Some("2018-02-13".into()),
            ids: WorkIds {
                openalex: Some("https://openalex.org/W2741809807".into()),
                doi: Some("https://doi.org/10.7717/peerj.4375".into()),
                pmid: Some("https://pubmed.ncbi.nlm.nih.gov/29456894".into()),
                ..Default::default()
            },
            language: Some("en".into()),
            type_: Some("article".into()),
            cited_by_count: 100,
            biblio: Some(Biblio {
                volume: Some("6".into()),
                issue: Some("e4375".into()),
                first_page: Some("e4375".into()),
                last_page: None,
            }),
            authorships: vec![
                Authorship {
                    author_position: Some("first".into()),
                    author: DehydratedAuthor {
                        id: Some("https://openalex.org/A1969205032".into()),
                        display_name: Some("Heather A Piwowar".into()),
                        orcid: Some("https://orcid.org/0000-0003-1613-5981".into()),
                    },
                    institutions: vec![DehydratedInstitution {
                        id: Some("https://openalex.org/I4200000001".into()),
                        display_name: Some("OurResearch".into()),
                        ..Default::default()
                    }],
                    ..Default::default()
                },
                Authorship {
                    author_position: Some("last".into()),
                    author: DehydratedAuthor {
                        id: None,
                        display_name: Some("Juan Pablo Alperin".into()),
                        orcid: Some("https://orcid.org/0000-0002-9344-7439".into()),
                    },
                    ..Default::default()
                },
            ],
            primary_location: Some(Location {
                source: Some(DehydratedSource {
                    id: Some("https://openalex.org/V1983995261".into()),
                    display_name: Some("PeerJ".into()),
                    issn_l: Some("2167-8359".into()),
                    ..Default::default()
                }),
                ..Default::default()
            }),
            topics: vec![TopicAssignment {
                id: "https://openalex.org/T10100".into(),
                display_name: "Open Access".into(),
                ..Default::default()
            }],
            keywords: vec![KeywordAssignment {
                id: "https://openalex.org/K1".into(),
                display_name: "open access".into(),
                ..Default::default()
            }],
            mesh: vec![MeshTerm {
                descriptor_name: Some("Peer Review, Research".into()),
                ..Default::default()
            }],
            ..Default::default()
        }
    }

    #[test]
    fn work_to_article_basic_fields() {
        let work = sample_work();
        let article = work_to_article(&work);

        assert_eq!(article.title, "The state of OA");
        assert_eq!(article.source, ArticleSource::OpenAlex);
        assert_eq!(article.year, Some(2018));
        assert_eq!(article.month, Some(2));
        assert_eq!(article.language.as_deref(), Some("en"));
        assert_eq!(article.journal.as_deref(), Some("PeerJ"));
        assert_eq!(article.issn.as_deref(), Some("2167-8359"));
    }

    #[test]
    fn identifiers_collected() {
        let work = sample_work();
        let article = work_to_article(&work);

        // Should have OpenAlex, DOI, and PMID identifiers.
        assert!(article.doi().is_some(), "should have DOI");
        assert_eq!(article.doi(), Some("10.7717/peerj.4375"));
        assert!(article.pmid().is_some(), "should have PMID");
        assert_eq!(
            article.identifier(IdKind::OpenAlex),
            Some("W2741809807"),
            "should have bare OpenAlex ID"
        );
    }

    #[test]
    fn authors_parsed() {
        let work = sample_work();
        let article = work_to_article(&work);

        assert_eq!(article.authors.len(), 2);
        // First author
        assert_eq!(article.authors[0].last_name, "Piwowar");
        assert_eq!(
            article.authors[0].orcid.as_deref(),
            Some("0000-0003-1613-5981")
        );
        assert_eq!(
            article.authors[0].affiliation.as_deref(),
            Some("OurResearch")
        );
        // Second author
        assert_eq!(article.authors[1].last_name, "Alperin");
    }

    #[test]
    fn biblio_mapped() {
        let work = sample_work();
        let article = work_to_article(&work);

        assert_eq!(article.volume.as_deref(), Some("6"));
        assert_eq!(article.issue.as_deref(), Some("e4375"));
        assert_eq!(article.pages.as_deref(), Some("e4375"));
    }

    #[test]
    fn pub_type_mapped() {
        let work = sample_work();
        let article = work_to_article(&work);
        assert!(article.pub_types.contains(&"article".to_string()));
    }

    #[test]
    fn keywords_from_topics_keywords_mesh() {
        let work = sample_work();
        let article = work_to_article(&work);

        assert!(article.keywords.contains(&"Open Access".to_string()));
        assert!(article.keywords.contains(&"open access".to_string()));
        assert!(article
            .keywords
            .contains(&"Peer Review, Research".to_string()));
    }

    #[test]
    fn minimal_work() {
        let work = Work {
            id: "https://openalex.org/W1".into(),
            ..Default::default()
        };
        let article = work_to_article(&work);

        assert_eq!(article.title, "(untitled)");
        assert_eq!(article.source, ArticleSource::OpenAlex);
        assert_eq!(
            article.identifier(IdKind::OpenAlex),
            Some("W1")
        );
        assert!(article.authors.is_empty());
        assert!(article.doi().is_none());
    }

    #[test]
    fn doi_normalised() {
        let work = Work {
            id: "https://openalex.org/W1".into(),
            doi: Some("doi:10.1000/test".into()),
            ..Default::default()
        };
        let article = work_to_article(&work);
        assert_eq!(article.doi(), Some("10.1000/test"));
    }

    #[test]
    fn abstract_reconstructed() {
        let mut inv = std::collections::BTreeMap::new();
        inv.insert("Hello".to_string(), vec![0u32]);
        inv.insert("world".to_string(), vec![1u32]);
        let work = Work {
            id: "https://openalex.org/W1".into(),
            abstract_inverted_index: Some(inv),
            ..Default::default()
        };
        let article = work_to_article(&work);
        assert_eq!(article.abstract_text.as_deref(), Some("Hello world"));
    }

    #[test]
    fn parse_iso_date_basic() {
        assert_eq!(parse_iso_date("2024-03-15"), (Some(2024), Some(3)));
        assert_eq!(parse_iso_date("2024"), (Some(2024), None));
        assert_eq!(parse_iso_date("nonsense"), (None, None));
    }

    #[test]
    fn works_to_articles_batch() {
        let resp = crate::types::ListResponse {
            meta: crate::types::Meta {
                count: 2,
                ..Default::default()
            },
            results: vec![
                Work {
                    id: "https://openalex.org/W1".into(),
                    doi: Some("https://doi.org/10.1/a".into()),
                    display_name: Some("First".into()),
                    ..Default::default()
                },
                Work {
                    id: "https://openalex.org/W2".into(),
                    display_name: Some("Second".into()),
                    ..Default::default()
                },
            ],
            group_by: vec![],
        };
        let articles = works_to_articles(&resp);
        assert_eq!(articles.len(), 2);
        assert_eq!(articles[0].title, "First");
        assert_eq!(articles[1].title, "Second");
    }
}
