//! Conversion layer: parse raw NCBI E-utility responses into typed
//! [`bib_types::Article`] records.
//!
//! ## ESummary JSON → Article
//!
//! ESummary v2.0 returns JSON shaped like:
//! ```json
//! {
//!   "result": {
//!     "uids": ["123"],
//!     "123": {
//!       "uid": "123", "title": "...", "pubdate": "2024 Jan 15",
//!       "authors": [{ "name": "Smith JA" }],
//!       "journal": { "title": "Nature", "volume": "1", "issue": "2" },
//!       "elocationid": "doi: 10.1000/test",
//!       "articleids": [{ "idtype": "doi", "value": "10.1000/test" }],
//!       "pubtype": ["Journal Article"],
//!       "abstract": "..."
//!     }
//!   }
//! }
//! ```
//!
//! ## MEDLINE text → Article
//!
//! EFetch with `rettype=medline` returns records with field tags:
//! ```text
//! PMID- 12345
//! TI  - Title here.
//! AB  - Abstract text.
//! FAU - Smith, John A
//! AU  - Smith JA
//! JT  - Journal Title
//! DP  - 2024 Jan 15
//! PT  - Journal Article
//! AID - 10.1000/test [doi]
//! MH  - keyword
//! LA  - ENG
//! IS  - 0028-0836
//! ```

use bib_types::convert::{build_article, normalize_doi, parse_author_name, parse_pubdate};
use bib_types::{Article, ArticleSource, Author, IdKind, Identifier};
use serde_json::Value;

// ---------------------------------------------------------------------------
// ESummary JSON → Vec<Article>
// ---------------------------------------------------------------------------

/// Parse an ESummary v2.0 JSON response into typed [`Article`]s.
///
/// Articles are returned in UID order. Records that are missing from the
/// response (e.g. invalid PMIDs) are silently skipped.
pub fn esummary_to_articles(json: &Value) -> Vec<Article> {
    let result = match json.get("result") {
        Some(v) => v,
        None => return Vec::new(),
    };

    let uids: Vec<&str> = result
        .get("uids")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(|v| v.as_str()).collect())
        .unwrap_or_default();

    uids.iter()
        .filter_map(|&pmid| result.get(pmid).map(|doc| parse_esummary_doc(pmid, doc)))
        .collect()
}

fn parse_esummary_doc(pmid: &str, doc: &Value) -> Article {
    let title = str_field(doc, "title");

    // Authors
    let authors = doc
        .get("authors")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|a| {
                    let name = a.get("name").and_then(|v| v.as_str()).unwrap_or("");
                    if name.is_empty() {
                        None
                    } else {
                        Some(parse_author_name(name))
                    }
                })
                .collect::<Vec<Author>>()
        })
        .unwrap_or_default();

    // Identifiers — collect DOI from elocationid and articleids, plus PMID
    let mut identifiers = vec![Identifier::pmid(pmid)];

    // elocationid often contains "doi: 10.xxx/yyy"
    let elo = str_field(doc, "elocationid");
    let doi_from_elocation = if elo.starts_with("doi:") {
        let d = elo.strip_prefix("doi:").unwrap_or(elo).trim();
        if !d.is_empty() {
            Some(d.to_owned())
        } else {
            None
        }
    } else {
        None
    };

    // articleids array may also have DOI, PMC, etc.
    if let Some(ids) = doc.get("articleids").and_then(|v| v.as_array()) {
        for entry in ids {
            let idtype = str_field(entry, "idtype");
            let value = str_field(entry, "value");
            if value.is_empty() {
                continue;
            }
            match idtype {
                "doi" => {
                    if !identifiers.iter().any(|i| i.kind == IdKind::Doi) {
                        identifiers.push(Identifier::doi(normalize_doi(value)));
                    }
                }
                "pmc" => {
                    let v = value.strip_prefix("PMC").unwrap_or(value);
                    identifiers.push(Identifier::new(IdKind::Pmc, v));
                }
                _ => {}
            }
        }
    }

    // Prefer elocation DOI if we didn't get one from articleids
    if !identifiers.iter().any(|i| i.kind == IdKind::Doi) {
        if let Some(d) = doi_from_elocation {
            identifiers.push(Identifier::doi(normalize_doi(&d)));
        }
    }

    let mut article = build_article(pmid, title, identifiers, authors, ArticleSource::Pubmed);

    // Journal info
    if let Some(journal) = doc.get("journal") {
        let jname = str_field(journal, "title");
        if !jname.is_empty() {
            article.journal = Some(jname.to_owned());
        }
        let vol = str_field(journal, "volume");
        if !vol.is_empty() {
            article.volume = Some(vol.to_owned());
        }
        let iss = str_field(journal, "issue");
        if !iss.is_empty() {
            article.issue = Some(iss.to_owned());
        }
    }

    // Full journal name fallback
    if article.journal.is_none() {
        let full = str_field(doc, "fulljournalname");
        if !full.is_empty() {
            article.journal = Some(full.to_owned());
        }
    }

    // Dates
    let pubdate = str_field(doc, "pubdate");
    if !pubdate.is_empty() {
        let (year, month) = parse_pubdate(pubdate);
        article.year = year;
        article.month = month;
    }

    // Abstract
    let abs = str_field(doc, "abstract");
    if !abs.is_empty() {
        article.abstract_text = Some(strip_html_tags(abs));
    }

    // Publication types
    article.pub_types = str_array(doc, "pubtype");

    // MeSH terms → keywords
    article.keywords = str_array(doc, "meshheadings");

    article
}

// ---------------------------------------------------------------------------
// MEDLINE text → Vec<Article>
// ---------------------------------------------------------------------------

/// Parse MEDLINE-formatted text (from EFetch `rettype=medline`) into typed
/// [`Article`]s.
///
/// Each record is delimited by a blank line. Fields are tagged with
/// 3- or 4-letter labels (`PMID-`, `TI  -`, `AB  -`, `FAU -`, etc.).
pub fn medline_to_articles(text: &str) -> Vec<Article> {
    text.split("\n\n")
        .filter(|block| block.contains("PMID- "))
        .map(parse_medline_record)
        .collect()
}

fn parse_medline_record(block: &str) -> Article {
    let fields = parse_medline_fields(block);

    let pmid = fields
        .iter()
        .find(|(k, _)| *k == "PMID")
        .map(|(_, v)| v.trim().to_owned())
        .unwrap_or_default();

    let title = fields
        .iter()
        .find(|(k, _)| *k == "TI")
        .map(|(_, v)| v.clone())
        .unwrap_or_default();

    // FAU (Full Author) preferred over AU
    let fau: Vec<&str> = fields
        .iter()
        .filter(|(k, _)| *k == "FAU")
        .map(|(_, v)| v.as_str())
        .collect();
    let authors: Vec<Author> = if !fau.is_empty() {
        fau.iter().map(|s| parse_fau(s)).collect()
    } else {
        fields
            .iter()
            .filter(|(k, _)| *k == "AU")
            .map(|(_, v)| parse_author_name(v))
            .collect()
    };

    // DOI from AID field: "10.1000/test [doi]"
    let doi = fields
        .iter()
        .filter(|(k, _)| *k == "AID")
        .find_map(|(_, v)| {
            // AID format: "value [type]"
            if v.contains("[doi]") {
                let d = v.split("[doi]").next().unwrap_or("").trim();
                if !d.is_empty() {
                    return Some(normalize_doi(d));
                }
            }
            None
        });

    let mut identifiers = vec![Identifier::pmid(&pmid)];
    if let Some(d) = doi {
        identifiers.push(Identifier::doi(d));
    }
    // PMC ID
    if let Some(pmc) = fields.iter().find(|(k, _)| *k == "PMC") {
        identifiers.push(Identifier::new(IdKind::Pmc, pmc.1.trim()));
    }

    let mut article = build_article(pmid, title, identifiers, authors, ArticleSource::Pubmed);

    // Abstract
    if let Some(ab) = fields.iter().find(|(k, _)| *k == "AB") {
        article.abstract_text = Some(ab.1.clone());
    }

    // Journal
    if let Some(jt) = fields.iter().find(|(k, _)| *k == "JT") {
        article.journal = Some(jt.1.clone());
    } else if let Some(ta) = fields.iter().find(|(k, _)| *k == "TA") {
        article.journal = Some(ta.1.clone());
    }

    // Volume / issue / pages
    if let Some(vi) = fields.iter().find(|(k, _)| *k == "VI") {
        article.volume = Some(vi.1.clone());
    }
    if let Some(ip) = fields.iter().find(|(k, _)| *k == "IP") {
        article.issue = Some(ip.1.clone());
    }
    if let Some(pg) = fields.iter().find(|(k, _)| *k == "PG") {
        article.pages = Some(pg.1.clone());
    }

    // Date: DP field "2024 Jan 15"
    if let Some(dp) = fields.iter().find(|(k, _)| *k == "DP") {
        let (year, month) = parse_pubdate(&dp.1);
        article.year = year;
        article.month = month;
    }

    // Publication types
    article.pub_types = fields
        .iter()
        .filter(|(k, _)| *k == "PT")
        .map(|(_, v)| v.clone())
        .collect();

    // MeSH headings — strip the `*` (major topic) marker but keep the
    // hierarchical path, e.g. `Genetics/*methods` → `Genetics/methods`.
    article.keywords = fields
        .iter()
        .filter(|(k, _)| *k == "MH")
        .map(|(_, v)| v.replace('*', "").trim().to_owned())
        .collect();

    // Language
    if let Some(la) = fields.iter().find(|(k, _)| *k == "LA") {
        article.language = Some(la.1.to_lowercase());
    }

    // ISSN
    if let Some(is) = fields.iter().find(|(k, _)| *k == "IS") {
        article.issn = Some(is.1.clone());
    }

    article
}

/// Parse a MEDLINE record block into `(tag, value)` pairs.
///
/// MEDLINE continuation lines start with `      ` (6 spaces). The field
/// value is the concatenation of the first line's value and all continuation
/// lines joined by a space.
fn parse_medline_fields(block: &str) -> Vec<(String, String)> {
    let mut fields = Vec::new();
    let mut current_tag: Option<String> = None;
    let mut current_val = String::new();

    let flush = |tag: &mut Option<String>, val: &mut String, fields: &mut Vec<_>| {
        if let Some(t) = tag.take() {
            fields.push((t, std::mem::take(val)));
        }
    };

    for line in block.lines() {
        if line.starts_with("      ") {
            // Continuation line — append to current field
            current_val.push(' ');
            current_val.push_str(line.trim());
        } else if line.len() >= 6 {
            // New field: "TAG - value" (tag is 2-4 chars, then " - ")
            let tag_part = line[..4].trim_end();
            let rest = &line[4..];
            let val = rest
                .strip_prefix(" - ")
                .or_else(|| rest.strip_prefix("- "))
                .unwrap_or(rest);
            flush(&mut current_tag, &mut current_val, &mut fields);
            current_tag = Some(tag_part.to_owned());
            current_val = val.trim().to_owned();
        } else {
            // Short line — flush current field
            flush(&mut current_tag, &mut current_val, &mut fields);
        }
    }
    flush(&mut current_tag, &mut current_val, &mut fields);
    fields
}

/// Parse a full-author string (MEDLINE `FAU` field) like
/// `"Smith, John A"` or `"Smith JA"` into an [`Author`].
fn parse_fau(s: &str) -> Author {
    let trimmed = s.trim();
    // "LastName, ForeName" format
    if let Some((last, fore)) = trimmed.split_once(',') {
        let last = last.trim();
        let fore = fore.trim();
        // Extract initials from fore_name (first letters of each part)
        let initials: String = fore
            .split_whitespace()
            .filter_map(|w| w.chars().next())
            .collect();
        return Author {
            last_name: last.to_owned(),
            fore_name: Some(fore.to_owned()),
            initials: if !initials.is_empty() {
                Some(initials)
            } else {
                None
            },
            affiliation: None,
            orcid: None,
            corresponding: false,
        };
    }
    // Fall back to "LastName Initials" format
    parse_author_name(trimmed)
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn str_field<'a>(v: &'a Value, key: &str) -> &'a str {
    v.get(key).and_then(|v| v.as_str()).unwrap_or("")
}

fn str_array(v: &Value, key: &str) -> Vec<String> {
    v.get(key)
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_owned()))
                .collect()
        })
        .unwrap_or_default()
}

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
    use serde_json::json;

    #[test]
    fn esummary_single_article() {
        let json = json!({
            "result": {
                "uids": ["12345"],
                "12345": {
                    "title": "A Great Paper",
                    "authors": [
                        { "name": "Smith JA", "authtype": "Author" },
                        { "name": "Jones B", "authtype": "Author" }
                    ],
                    "journal": { "title": "Nature", "volume": "1", "issue": "2" },
                    "pubdate": "2024 Jan 15",
                    "elocationid": "doi: 10.1234/test",
                    "pubtype": ["Journal Article", "Research Support, Non-U.S. Gov't"],
                    "abstract": "This is the <b>abstract</b> text."
                }
            }
        });

        let articles = esummary_to_articles(&json);
        assert_eq!(articles.len(), 1);

        let a = &articles[0];
        assert_eq!(a.title, "A Great Paper");
        assert_eq!(a.pmid(), Some("12345"));
        assert_eq!(a.doi(), Some("10.1234/test"));
        assert_eq!(a.authors.len(), 2);
        assert_eq!(a.authors[0].last_name, "Smith");
        assert_eq!(a.authors[0].initials.as_deref(), Some("JA"));
        assert_eq!(a.journal.as_deref(), Some("Nature"));
        assert_eq!(a.volume.as_deref(), Some("1"));
        assert_eq!(a.issue.as_deref(), Some("2"));
        assert_eq!(a.year, Some(2024));
        assert_eq!(a.month, Some(1));
        assert_eq!(
            a.abstract_text.as_deref(),
            Some("This is the abstract text.")
        );
        assert!(a.pub_types.contains(&"Journal Article".to_owned()));
        assert_eq!(a.source, ArticleSource::Pubmed);
    }

    #[test]
    fn esummary_doi_from_articleids() {
        let json = json!({
            "result": {
                "uids": ["999"],
                "999": {
                    "title": "Test",
                    "articleids": [
                        { "idtype": "pubmed", "value": "999" },
                        { "idtype": "doi", "value": "10.2000/fromids" },
                        { "idtype": "pmc", "value": "PMC777" }
                    ]
                }
            }
        });

        let articles = esummary_to_articles(&json);
        let a = &articles[0];
        assert_eq!(a.doi(), Some("10.2000/fromids"));
        assert_eq!(a.identifier(IdKind::Pmc), Some("777"));
    }

    #[test]
    fn esummary_missing_record_skipped() {
        let json = json!({
            "result": {
                "uids": ["123", "000"],
                "123": { "title": "OK" }
            }
        });
        let articles = esummary_to_articles(&json);
        assert_eq!(articles.len(), 1);
        assert_eq!(articles[0].title, "OK");
    }

    #[test]
    fn esummary_empty() {
        let json = json!({ "result": { "uids": [] } });
        assert!(esummary_to_articles(&json).is_empty());
    }

    #[test]
    fn medline_single_record() {
        let medline = "\
PMID- 12345
TI  - A breakthrough in genomics research.
AB  - This is the full abstract text that spans
      multiple lines for readability.
FAU - Smith, John A
AU  - Smith JA
FAU - Jones, Bob
AU  - Jones B
JT  - Nature Genetics
DP  - 2024 Mar 15
VI  - 56
IP  - 3
PG  - 100-110
PT  - Journal Article
PT  - Research Support, N.I.H., Extramural
MH  - Genetics/*methods
MH  - Genome-Wide Association Study
LA  - ENG
IS  - 1061-4036
AID - 10.1038/s41588-024-01234-5 [doi]
";

        let articles = medline_to_articles(medline);
        assert_eq!(articles.len(), 1);

        let a = &articles[0];
        assert_eq!(a.pmid(), Some("12345"));
        assert_eq!(a.title, "A breakthrough in genomics research.");
        assert_eq!(a.doi(), Some("10.1038/s41588-024-01234-5"));
        assert_eq!(a.authors.len(), 2);
        assert_eq!(a.authors[0].last_name, "Smith");
        assert_eq!(a.authors[0].fore_name.as_deref(), Some("John A"));
        assert_eq!(a.authors[0].initials.as_deref(), Some("JA"));
        assert_eq!(a.authors[1].last_name, "Jones");
        assert_eq!(a.journal.as_deref(), Some("Nature Genetics"));
        assert_eq!(a.volume.as_deref(), Some("56"));
        assert_eq!(a.issue.as_deref(), Some("3"));
        assert_eq!(a.pages.as_deref(), Some("100-110"));
        assert_eq!(a.year, Some(2024));
        assert_eq!(a.month, Some(3));
        assert!(a.abstract_text.as_ref().unwrap().contains("multiple lines"));
        assert_eq!(a.pub_types.len(), 2);
        assert_eq!(a.keywords.len(), 2);
        assert_eq!(a.keywords[0], "Genetics/methods");
        assert_eq!(a.language.as_deref(), Some("eng"));
        assert_eq!(a.issn.as_deref(), Some("1061-4036"));
    }

    #[test]
    fn medline_multiple_records() {
        let medline = "\
PMID- 111
TI  - First paper.
AU  - Alpha A

PMID- 222
TI  - Second paper.
AU  - Beta B
";
        let articles = medline_to_articles(medline);
        assert_eq!(articles.len(), 2);
        assert_eq!(articles[0].title, "First paper.");
        assert_eq!(articles[1].title, "Second paper.");
    }

    #[test]
    fn medline_no_doi_still_works() {
        let medline = "\
PMID- 333
TI  - Old paper without DOI.
AU  - Gamma G
DP  - 2001
";
        let articles = medline_to_articles(medline);
        assert_eq!(articles.len(), 1);
        let a = &articles[0];
        assert!(a.doi().is_none());
        assert_eq!(a.pmid(), Some("333"));
        assert_eq!(a.year, Some(2001));
    }
}
