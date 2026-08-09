//! Conversion layer: transform bioRxiv/medRxiv API entries into typed
//! [`bib_types::Article`] records.
//!
//! ## Author parsing
//!
//! The API returns authors as a single semicolon-separated string:
//! `"Watson, O. J.; Tran, T. N.-A.; Zupko, R. J."`.
//! Each entry follows `"Last, F. M."` convention (family name, then
//! space-separated initials each ending with a period).

use bib_types::convert::{build_article, normalize_doi};
use bib_types::{Article, ArticleSource, Author, IdKind, Identifier};

use crate::types::BiorxivEntry;

// ---------------------------------------------------------------------------
// Entry → Article
// ---------------------------------------------------------------------------

/// Convert a single [`BiorxivEntry`] into an [`Article`].
///
/// The DOI is normalised and stored as both a [`IdKind::Doi`] and a
/// [`IdKind::Biorxiv`] identifier (the bioRxiv/medRxiv DOI namespace).
/// If a published DOI is present, it is added as a separate [`IdKind::Doi`].
pub fn entry_to_article(entry: &BiorxivEntry) -> Article {
    let doi = normalize_doi(&entry.doi);

    let mut identifiers = Vec::new();
    if !doi.is_empty() {
        identifiers.push(Identifier::doi(&doi));
        identifiers.push(Identifier::new(IdKind::Biorxiv, &doi));
    }

    // Published-version DOI (journal article).
    let published_doi = normalize_doi(&entry.published);
    if !published_doi.is_empty() && published_doi != "NA" {
        identifiers.push(Identifier::doi(&published_doi));
    }

    let authors = parse_biorxiv_authors(
        &entry.authors,
        entry.author_corresponding.as_deref(),
        entry.author_corresponding_institution.as_deref(),
    );

    let mut article = build_article(
        if doi.is_empty() { &entry.title } else { &doi },
        &entry.title,
        identifiers,
        authors,
        ArticleSource::Biorxiv,
    );

    // Abstract.
    let abstract_text = entry.abstract_text.trim();
    if !abstract_text.is_empty() && abstract_text != "NA" {
        article.abstract_text = Some(abstract_text.to_owned());
    }

    // Posting date → year, month.
    let (year, month) = parse_date(&entry.date);
    article.year = year;
    article.month = month;

    // Category and type as pub_types / keywords.
    let category = entry.category.trim();
    if !category.is_empty() && category != "NA" {
        article.pub_types.push(category.to_owned());
    }

    let atype = entry.article_type.trim();
    if !atype.is_empty() && atype != "NA" {
        article.pub_types.push(atype.to_owned());
    }

    // License as a keyword for provenance.
    let license = entry.license.trim();
    if !license.is_empty() && license != "NA" {
        article.keywords.push(format!("license:{license}"));
    }

    article
}

/// Convert a slice of [`BiorxivEntry`] into [`Vec<Article>`].
pub fn entries_to_articles(entries: &[BiorxivEntry]) -> Vec<Article> {
    entries.iter().map(entry_to_article).collect()
}

// ---------------------------------------------------------------------------
// Version helpers
// ---------------------------------------------------------------------------

/// Pick the entry with the highest version number from a list.
///
/// When fetching by DOI, the API returns all posted versions. This helper
/// selects the latest.
pub fn latest_version(entries: &[BiorxivEntry]) -> Option<&BiorxivEntry> {
    entries
        .iter()
        .max_by_key(|e| e.version.trim().parse::<u32>().unwrap_or(0))
}

// ---------------------------------------------------------------------------
// Author parsing
// ---------------------------------------------------------------------------

/// Parse the bioRxiv/medRxiv authors string.
///
/// Input format: `"Watson, O. J.; Tran, T. N.-A.; Zupko, R. J."`
///
/// Each semicolon-separated entry is `"Last, F. M."` — family name, comma,
/// then space-separated initials (each a single letter + period). We store
/// the family name in `last_name` and the concatenated bare initials (e.g.
/// `"OJ"`) in `initials`.
///
/// If a corresponding-author name and institution are provided, the matching
/// author (by last name) is flagged `corresponding = true` and given the
/// institution affiliation.
fn parse_biorxiv_authors(
    authors_str: &str,
    corresponding: Option<&str>,
    institution: Option<&str>,
) -> Vec<Author> {
    let corresponding_last = corresponding.map(extract_last_name_from_full);

    authors_str
        .split(';')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|entry| parse_single_author(entry, corresponding_last, institution))
        .collect()
}

/// Parse one `"Last, F. M."` entry into an [`Author`].
fn parse_single_author(
    entry: &str,
    corresponding_last: Option<&str>,
    institution: Option<&str>,
) -> Author {
    let (last_name, initials_part) = match entry.split_once(',') {
        Some((last, rest)) => (last.trim(), rest.trim()),
        None => (entry.trim(), ""),
    };

    // Convert "O. J." → "OJ" and "T. N.-A." → "TNA" by collecting all
    // alphabetic characters (periods, hyphens, spaces are discarded).
    let initials: String = initials_part
        .chars()
        .filter(|c| c.is_ascii_alphabetic())
        .collect();

    // Match corresponding author by last name (case-insensitive).
    let is_corresponding = corresponding_last.is_some_and(|cl| last_name.eq_ignore_ascii_case(cl));

    Author {
        last_name: last_name.to_owned(),
        fore_name: None,
        initials: if initials.is_empty() {
            None
        } else {
            Some(initials)
        },
        affiliation: if is_corresponding {
            institution
                .filter(|s| !s.is_empty() && *s != "NA")
                .map(|s| s.to_owned())
        } else {
            None
        },
        orcid: None,
        corresponding: is_corresponding,
    }
}

/// Extract the family name from a full display name like `"Oliver J. Watson"`.
/// Convention: the last whitespace-separated token is the family name.
fn extract_last_name_from_full(name: &str) -> &str {
    name.split_whitespace().last().unwrap_or(name)
}

// ---------------------------------------------------------------------------
// Date parsing
// ---------------------------------------------------------------------------

/// Parse a `"YYYY-MM-DD"` date into `(year, month)`.
fn parse_date(s: &str) -> (Option<u16>, Option<u8>) {
    let s = s.trim();
    if s.len() < 4 {
        return (None, None);
    }
    let year = s.get(0..4).and_then(|y| y.parse::<u16>().ok());
    let month = s
        .get(5..7)
        .and_then(|m| m.parse::<u8>().ok())
        .filter(|m| (1..=12).contains(m));
    (year, month)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_entry() -> BiorxivEntry {
        BiorxivEntry {
            doi: "10.1101/2023.10.21.23297352".into(),
            title: "Global risk of pfhrp2/3 gene deletions".into(),
            authors: "Watson, O. J.; Tran, T. N.-A.; Zupko, R. J.".into(),
            author_corresponding: Some("Oliver J. Watson".into()),
            author_corresponding_institution: Some("Imperial College London".into()),
            date: "2024-01-01".into(),
            version: "3".into(),
            article_type: "new_result".into(),
            license: "cc_by_nd".into(),
            category: "infectious diseases".into(),
            jatsxml: "https://www.medrxiv.org/content/early/2024/01/01/source.xml".into(),
            abstract_text: "Background pfhrp2 deletions threaten malaria control.".into(),
            published: "NA".into(),
            server: "medrxiv".into(),
            funder: "NA".into(),
        }
    }

    #[test]
    fn parse_authors_basic() {
        let authors =
            parse_biorxiv_authors("Watson, O. J.; Tran, T. N.-A.; Zupko, R. J.", None, None);
        assert_eq!(authors.len(), 3);

        assert_eq!(authors[0].last_name, "Watson");
        assert_eq!(authors[0].initials.as_deref(), Some("OJ"));
        assert!(!authors[0].corresponding); // no corresponding info → false

        assert_eq!(authors[1].last_name, "Tran");
        assert_eq!(authors[1].initials.as_deref(), Some("TNA"));
        assert!(!authors[1].corresponding);

        assert_eq!(authors[2].last_name, "Zupko");
        assert_eq!(authors[2].initials.as_deref(), Some("RJ"));
    }

    #[test]
    fn parse_authors_with_corresponding() {
        let authors = parse_biorxiv_authors(
            "Watson, O. J.; Tran, T. N.-A.",
            Some("Oliver J. Watson"),
            Some("Imperial College London"),
        );
        assert!(authors[0].corresponding);
        assert_eq!(
            authors[0].affiliation.as_deref(),
            Some("Imperial College London")
        );
        assert!(!authors[1].corresponding);
        assert!(authors[1].affiliation.is_none());
    }

    #[test]
    fn parse_authors_hyphenated_initials() {
        let authors = parse_biorxiv_authors("Tran, T. N.-A.", None, None);
        assert_eq!(authors[0].last_name, "Tran");
        // "T. N.-A." → all alphabetic chars: T, N, A → "TNA"
        assert_eq!(authors[0].initials.as_deref(), Some("TNA"));
    }

    #[test]
    fn entry_to_article_full() {
        let entry = sample_entry();
        let article = entry_to_article(&entry);

        assert_eq!(article.title, "Global risk of pfhrp2/3 gene deletions");
        assert_eq!(article.doi(), Some("10.1101/2023.10.21.23297352"));
        assert_eq!(
            article.identifier(IdKind::Biorxiv),
            Some("10.1101/2023.10.21.23297352")
        );
        assert_eq!(article.authors.len(), 3);
        assert_eq!(article.authors[0].last_name, "Watson");
        assert_eq!(article.year, Some(2024));
        assert_eq!(article.month, Some(1));
        assert_eq!(article.source, ArticleSource::Biorxiv);
        assert!(
            article
                .pub_types
                .contains(&"infectious diseases".to_owned())
        );
        assert!(article.pub_types.contains(&"new_result".to_owned()));
        assert!(article.abstract_text.as_ref().unwrap().contains("pfhrp2"));
    }

    #[test]
    fn entry_with_published_doi() {
        let mut entry = sample_entry();
        entry.published = "10.1007/s12020-024-03982-2".into();
        let article = entry_to_article(&entry);
        // Should have DOI, Biorxiv, and published DOI identifiers.
        assert_eq!(article.identifiers.len(), 3);
    }

    #[test]
    fn latest_version_picks_highest() {
        let mut e1 = sample_entry();
        e1.version = "1".into();
        let mut e2 = sample_entry();
        e2.version = "3".into();
        let mut e3 = sample_entry();
        e3.version = "2".into();

        let entries = [e1, e2, e3];
        let latest = latest_version(&entries).unwrap();
        assert_eq!(latest.version, "3");
    }

    #[test]
    fn parse_date_basic() {
        assert_eq!(parse_date("2024-01-15"), (Some(2024), Some(1)));
        assert_eq!(parse_date("2024-12-31"), (Some(2024), Some(12)));
        assert_eq!(parse_date("2024"), (Some(2024), None));
        assert_eq!(parse_date(""), (None, None));
    }

    #[test]
    fn extract_last_name() {
        assert_eq!(extract_last_name_from_full("Oliver J. Watson"), "Watson");
        assert_eq!(extract_last_name_from_full("Vinod Scaria"), "Scaria");
        assert_eq!(extract_last_name_from_full("Galileo"), "Galileo");
    }

    #[test]
    fn parse_authors_empty() {
        let authors = parse_biorxiv_authors("", None, None);
        assert!(authors.is_empty());
    }

    #[test]
    fn parse_authors_single_name_no_comma() {
        let authors = parse_biorxiv_authors("Galileo", None, None);
        assert_eq!(authors.len(), 1);
        assert_eq!(authors[0].last_name, "Galileo");
        assert!(authors[0].initials.is_none());
    }
}
