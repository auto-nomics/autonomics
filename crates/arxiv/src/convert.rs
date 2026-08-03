//! Conversion layer: parse arXiv Atom XML responses into typed
//! [`bib_types::Article`] records and intermediate [`ArxivEntry`] structs.
//!
//! ## Atom XML → ArxivEntry / Article
//!
//! The arXiv API returns an Atom 1.0 feed with three namespaces:
//! - Default Atom: `http://www.w3.org/2005/Atom`
//! - OpenSearch: `http://a9.com/-/spec/opensearch/1.1/`
//! - arXiv extension: `http://arxiv.org/schemas/atom`
//!
//! We use `quick-xml`'s streaming reader, matching on local tag names
//! (ignoring namespace prefixes) for robustness.

use quick_xml::Reader;
use quick_xml::events::Event;

use bib_types::convert::{build_article, normalize_doi};
use bib_types::{Article, ArticleSource, Author, IdKind, Identifier};

use crate::error::{ArxivError, Result};
use crate::types::{ArxivAuthor, ArxivEntry, SearchResponse};

// ---------------------------------------------------------------------------
// Feed-level parser: Atom XML → SearchResponse
// ---------------------------------------------------------------------------

/// Parse a complete arXiv Atom XML feed into a [`SearchResponse`].
///
/// This is the main entry point used by [`crate::ArxivClient`].
pub fn parse_feed(xml: &str) -> Result<SearchResponse> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);

    let mut buf = Vec::new();

    let mut total_results: u64 = 0;
    let mut start_index: u64 = 0;
    let mut items_per_page: u64 = 0;
    let mut entries: Vec<ArxivEntry> = Vec::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => {
                let local = local_name(e.name().into_inner());
                match local {
                    "entry" => {
                        let entry = parse_entry(&mut reader)?;
                        entries.push(entry);
                    }
                    "totalResults" => {
                        total_results = read_text_u64(&mut reader)?;
                    }
                    "startIndex" => {
                        start_index = read_text_u64(&mut reader)?;
                    }
                    "itemsPerPage" => {
                        items_per_page = read_text_u64(&mut reader)?;
                    }
                    _ => {}
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => {
                return Err(ArxivError::Xml(format!("XML parse error: {e}")));
            }
            _ => {}
        }
        buf.clear();
    }

    Ok(SearchResponse {
        total_results,
        start_index,
        items_per_page,
        entries,
    })
}

// ---------------------------------------------------------------------------
// Entry parser: <entry>…</entry> → ArxivEntry
// ---------------------------------------------------------------------------

/// Parse a single `<entry>` element. Assumes the `<entry>` start tag has
/// already been consumed.
fn parse_entry(reader: &mut Reader<&[u8]>) -> Result<ArxivEntry> {
    let mut buf = Vec::new();
    let mut text_buf = String::new();

    let mut arxiv_id = String::new();
    let mut title = String::new();
    let mut summary: Option<String> = None;
    let mut authors: Vec<ArxivAuthor> = Vec::new();
    let mut published: Option<String> = None;
    let mut updated: Option<String> = None;
    let mut primary_category: Option<String> = None;
    let mut categories: Vec<String> = Vec::new();
    let mut doi: Option<String> = None;
    let mut journal_ref: Option<String> = None;
    let mut comment: Option<String> = None;
    let mut pdf_url: Option<String> = None;
    let mut abs_url: Option<String> = None;

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => {
                let local = local_name(e.name().into_inner());
                match local {
                    "author" => {
                        authors.push(parse_author(reader)?);
                    }
                    "link" => {
                        process_link(&e, &mut pdf_url, &mut abs_url);
                    }
                    "category" => {
                        process_category(&e, &mut categories);
                    }
                    "primary_category" => {
                        process_primary_category(&e, &mut primary_category);
                    }
                    _ => {}
                }
            }
            Ok(Event::Empty(e)) => {
                let local = local_name(e.name().into_inner());
                match local {
                    "link" => {
                        process_link(&e, &mut pdf_url, &mut abs_url);
                    }
                    "category" => {
                        process_category(&e, &mut categories);
                    }
                    "primary_category" => {
                        process_primary_category(&e, &mut primary_category);
                    }
                    _ => {}
                }
            }
            Ok(Event::Text(e)) => {
                text_buf.push_str(&decode_xml_text(&e.into_inner()));
            }
            Ok(Event::End(e)) => {
                let local = local_name(e.name().into_inner());
                match local {
                    "id" => {
                        arxiv_id = extract_arxiv_id(&text_buf);
                    }
                    "title" => {
                        title = collapse_whitespace(&std::mem::take(&mut text_buf));
                    }
                    "summary" => {
                        summary = Some(collapse_whitespace(&std::mem::take(&mut text_buf)));
                    }
                    "published" => {
                        published = Some(std::mem::take(&mut text_buf));
                    }
                    "updated" => {
                        updated = Some(std::mem::take(&mut text_buf));
                    }
                    "doi" => {
                        doi = Some(normalize_doi(&text_buf));
                    }
                    "journal_ref" => {
                        journal_ref = Some(std::mem::take(&mut text_buf));
                    }
                    "comment" => {
                        comment = Some(std::mem::take(&mut text_buf));
                    }
                    "entry" => break,
                    _ => {}
                }
                text_buf.clear();
            }
            Ok(Event::Eof) => {
                return Err(ArxivError::Xml("unexpected EOF inside <entry>".to_string()));
            }
            Err(e) => {
                return Err(ArxivError::Xml(format!("entry parse error: {e}")));
            }
            _ => {}
        }
        buf.clear();
    }

    Ok(ArxivEntry {
        arxiv_id,
        title,
        summary,
        authors,
        published,
        updated,
        primary_category,
        categories,
        doi,
        journal_ref,
        comment,
        pdf_url,
        abs_url,
    })
}

// ---------------------------------------------------------------------------
// Author parser: <author><name>…</name>[<affiliation>…</affiliation>]</author>
// ---------------------------------------------------------------------------

fn parse_author(reader: &mut Reader<&[u8]>) -> Result<ArxivAuthor> {
    let mut buf = Vec::new();
    let mut text_buf = String::new();

    let mut name = String::new();
    let mut affiliation: Option<String> = None;

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Text(e)) => {
                text_buf.push_str(&decode_xml_text(&e.into_inner()));
            }
            Ok(Event::End(e)) => {
                let local = local_name(e.name().into_inner());
                match local {
                    "name" => {
                        name = std::mem::take(&mut text_buf);
                    }
                    "affiliation" => {
                        affiliation = Some(std::mem::take(&mut text_buf));
                    }
                    "author" => break,
                    _ => {}
                }
                text_buf.clear();
            }
            Ok(Event::Eof) => {
                return Err(ArxivError::Xml(
                    "unexpected EOF inside <author>".to_string(),
                ));
            }
            Err(e) => {
                return Err(ArxivError::Xml(format!("author parse error: {e}")));
            }
            _ => {}
        }
        buf.clear();
    }

    Ok(ArxivAuthor { name, affiliation })
}

// ---------------------------------------------------------------------------
// Element helpers for Start/Empty events
// ---------------------------------------------------------------------------

/// Extract href/rel/title from a `<link>` element and update pdf_url/abs_url.
fn process_link(
    e: &quick_xml::events::BytesStart,
    pdf_url: &mut Option<String>,
    abs_url: &mut Option<String>,
) {
    let mut href = String::new();
    let mut rel = String::new();
    let mut title_attr = String::new();
    for attr in e.attributes().flatten() {
        let key = local_name(attr.key.into_inner());
        let val = decode_xml_text(&attr.value);
        match key {
            "href" => href = val,
            "rel" => rel = val,
            "title" => title_attr = val,
            _ => {}
        }
    }
    if rel == "alternate" && abs_url.is_none() {
        *abs_url = Some(href);
    } else if rel == "related" && title_attr == "pdf" {
        *pdf_url = Some(href);
    }
}

/// Extract the `term` attribute from a `<category>` element.
fn process_category(e: &quick_xml::events::BytesStart, categories: &mut Vec<String>) {
    for attr in e.attributes().flatten() {
        if local_name(attr.key.into_inner()) == "term" {
            categories.push(decode_xml_text(&attr.value));
        }
    }
}

/// Extract the `term` attribute from a `<primary_category>` element.
fn process_primary_category(
    e: &quick_xml::events::BytesStart,
    primary_category: &mut Option<String>,
) {
    for attr in e.attributes().flatten() {
        if local_name(attr.key.into_inner()) == "term" {
            *primary_category = Some(decode_xml_text(&attr.value));
        }
    }
}

// ---------------------------------------------------------------------------
// Atom XML → Vec<Article> (shared bib-types model)
// ---------------------------------------------------------------------------

/// Convert a list of [`ArxivEntry`]s into typed [`Article`] records.
///
/// Each entry's arXiv ID becomes an [`Identifier`] with [`IdKind::Arxiv`].
/// DOI and journal reference are extracted when present.
pub fn atom_to_articles(entries: &[ArxivEntry]) -> Vec<Article> {
    entries.iter().map(entry_to_article).collect()
}

/// Convert a single [`ArxivEntry`] into an [`Article`].
pub fn entry_to_article(entry: &ArxivEntry) -> Article {
    let mut identifiers = vec![Identifier::new(IdKind::Arxiv, &entry.arxiv_id)];
    if let Some(ref d) = entry.doi {
        let normalized = normalize_doi(d);
        if !normalized.is_empty() {
            identifiers.push(Identifier::doi(normalized));
        }
    }

    let authors: Vec<Author> = entry
        .authors
        .iter()
        .map(|a| parse_arxiv_author(&a.name, a.affiliation.as_deref()))
        .collect();

    let mut article = build_article(
        &entry.arxiv_id,
        &entry.title,
        identifiers,
        authors,
        ArticleSource::Arxiv,
    );

    article.abstract_text = entry.summary.clone();

    // Parse dates: ISO-8601 like "2024-01-15T00:00:00Z"
    if let Some(ref published) = entry.published {
        let (year, month) = parse_iso_date(published);
        article.year = year;
        article.month = month;
    }

    // Journal reference → journal field
    if let Some(ref jr) = entry.journal_ref {
        if !jr.is_empty() {
            article.journal = Some(jr.clone());
        }
    }

    // Comment → keywords (may contain submission info)
    if let Some(ref comment) = entry.comment {
        if !comment.is_empty() {
            article.keywords.push(comment.clone());
        }
    }

    // Primary category as a publication type
    if let Some(ref cat) = entry.primary_category {
        article.pub_types.push(cat.clone());
    }

    // All categories as keywords
    for cat in &entry.categories {
        if Some(cat.as_str()) != entry.primary_category.as_deref() {
            article.keywords.push(cat.clone());
        }
    }

    article
}

/// Parse an arXiv author name into a bib_types::Author.
///
/// arXiv names are typically `"FirstName LastName"` or `"F. Last Name"` with
/// the family name **last**. We split on the last space to extract the
/// family name.
fn parse_arxiv_author(name: &str, affiliation: Option<&str>) -> Author {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Author {
            last_name: String::new(),
            fore_name: None,
            initials: None,
            affiliation: affiliation.map(|s| s.to_owned()),
            orcid: None,
            corresponding: false,
        };
    }

    // arXiv convention: family name is last token.
    match trimmed.rsplit_once(' ') {
        Some((given, family)) if !family.is_empty() => {
            let initials: String = given
                .split_whitespace()
                .filter_map(|w| w.chars().next())
                .collect();
            Author {
                last_name: family.to_owned(),
                fore_name: Some(given.to_owned()),
                initials: if !initials.is_empty() {
                    Some(initials)
                } else {
                    None
                },
                affiliation: affiliation.map(|s| s.to_owned()),
                orcid: None,
                corresponding: false,
            }
        }
        _ => Author {
            last_name: trimmed.to_owned(),
            fore_name: None,
            initials: None,
            affiliation: affiliation.map(|s| s.to_owned()),
            orcid: None,
            corresponding: false,
        },
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Decode XML-encoded text: convert raw bytes to a UTF-8 string and replace
/// the five predefined XML entities (`&amp;`, `&lt;`, `&gt;`, `&quot;`,
/// `&apos;`).
fn decode_xml_text(bytes: &[u8]) -> String {
    let s = String::from_utf8_lossy(bytes);
    s.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
}

/// Extract the local part of an XML tag name, stripping any namespace
/// prefix (e.g. `arxiv:doi` → `doi`, `opensearch:totalResults` →
/// `totalResults`).
fn local_name(name: &[u8]) -> &str {
    let s = std::str::from_utf8(name).unwrap_or("");
    match s.rsplit_once(':') {
        Some((_prefix, local)) => local,
        None => s,
    }
}

/// Read the text content of the current element as a u64.
fn read_text_u64(reader: &mut Reader<&[u8]>) -> Result<u64> {
    let text = read_element_text(reader)?;
    text.trim()
        .parse::<u64>()
        .map_err(|_| ArxivError::Xml(format!("failed to parse u64 from '{text}'")))
}

/// Read text until the element's End event, then return it.
fn read_element_text(reader: &mut Reader<&[u8]>) -> Result<String> {
    let mut buf = Vec::new();
    let mut text = String::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Text(e)) => {
                text.push_str(&decode_xml_text(&e.into_inner()));
            }
            Ok(Event::End(_)) => break,
            Ok(Event::Eof) => break,
            Err(e) => return Err(ArxivError::Xml(format!("text read error: {e}"))),
            _ => {}
        }
        buf.clear();
    }
    Ok(text)
}

/// Extract the arXiv ID from an `<id>` URL like
/// `http://arxiv.org/abs/2401.12345v2`.
///
/// Also handles error URLs (`http://arxiv.org/api/errors#...`) by converting
/// them to the `api/errors#…` form for downstream detection.
///
/// Falls back to the raw text if it doesn't look like a URL.
fn extract_arxiv_id(id_text: &str) -> String {
    let trimmed = id_text.trim();
    // Normal arXiv abstract URL.
    for prefix in &["http://arxiv.org/abs/", "https://arxiv.org/abs/"] {
        if let Some(rest) = trimmed.strip_prefix(prefix) {
            return rest.to_owned();
        }
    }
    // Error entry URL — preserve the "api/errors" sentinel for detection.
    for prefix in &[
        "http://arxiv.org/api/errors#",
        "https://arxiv.org/api/errors#",
    ] {
        if let Some(rest) = trimmed.strip_prefix(prefix) {
            return format!("api/errors#{rest}");
        }
    }
    trimmed.to_owned()
}

/// Collapse multiple whitespace characters into a single space and trim.
fn collapse_whitespace(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Parse an ISO-8601 date string like `"2024-01-15T..."` into (year, month).
fn parse_iso_date(s: &str) -> (Option<u16>, Option<u8>) {
    let year = s.get(0..4).and_then(|y| y.parse::<u16>().ok());
    let month = s.get(5..7).and_then(|m| m.parse::<u8>().ok());
    (year, month)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_FEED: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<feed xmlns="http://www.w3.org/2005/Atom" xmlns:arxiv="http://arxiv.org/schemas/atom" xmlns:opensearch="http://a9.com/-/spec/opensearch/1.1/">
  <id>http://arxiv.org/api/query?search_query=ti:test</id>
  <updated>2024-01-20T00:00:00Z</updated>
  <title>arXiv Query: ti:test</title>
  <opensearch:totalResults xmlns:opensearch="http://a9.com/-/spec/opensearch/1.1/">2</opensearch:totalResults>
  <opensearch:startIndex xmlns:opensearch="http://a9.com/-/spec/opensearch/1.1/">0</opensearch:startIndex>
  <opensearch:itemsPerPage xmlns:opensearch="http://a9.com/-/spec/opensearch/1.1/">2</opensearch:itemsPerPage>
  <entry>
    <id>http://arxiv.org/abs/2401.12345v2</id>
    <updated>2024-01-20T12:00:00Z</updated>
    <published>2024-01-15T00:00:00Z</published>
    <title>Attention Is All You Need: A Comprehensive Survey of Transformer
    Architectures</title>
    <summary>This paper surveys transformer architectures and their
    applications across natural language processing, computer vision,
    and beyond.</summary>
    <author>
      <name>Ashish Vaswani</name>
      <arxiv:affiliation xmlns:arxiv="http://arxiv.org/schemas/atom">Google Brain</arxiv:affiliation>
    </author>
    <author>
      <name>Noam Shazeer</name>
    </author>
    <link href="http://arxiv.org/abs/2401.12345v2" rel="alternate" type="text/html"/>
    <link title="pdf" href="http://arxiv.org/pdf/2401.12345v2" rel="related" type="application/pdf"/>
    <link title="doi" href="http://dx.doi.org/10.1000/test" rel="related"/>
    <arxiv:primary_category xmlns:arxiv="http://arxiv.org/schemas/atom" term="cs.LG" scheme="http://arxiv.org/schemas/atom"/>
    <category term="cs.LG" scheme="http://arxiv.org/schemas/atom"/>
    <category term="cs.CL" scheme="http://arxiv.org/schemas/atom"/>
    <arxiv:comment xmlns:arxiv="http://arxiv.org/schemas/atom">15 pages, 3 figures</arxiv:comment>
    <arxiv:journal_ref xmlns:arxiv="http://arxiv.org/schemas/atom">Nature 2024</arxiv:journal_ref>
    <arxiv:doi xmlns:arxiv="http://arxiv.org/schemas/atom">10.1000/test</arxiv:doi>
  </entry>
  <entry>
    <id>http://arxiv.org/abs/2309.01234v1</id>
    <updated>2023-09-05T00:00:00Z</updated>
    <published>2023-09-01T00:00:00Z</published>
    <title>A Short Paper</title>
    <summary>Brief summary.</summary>
    <author>
      <name>Jane Doe</name>
    </author>
    <link href="http://arxiv.org/abs/2309.01234v1" rel="alternate" type="text/html"/>
    <link title="pdf" href="http://arxiv.org/pdf/2309.01234v1" rel="related" type="application/pdf"/>
    <arxiv:primary_category xmlns:arxiv="http://arxiv.org/schemas/atom" term="stat.ML" scheme="http://arxiv.org/schemas/atom"/>
  </entry>
</feed>"#;

    #[test]
    fn parse_feed_metadata() {
        let resp = parse_feed(SAMPLE_FEED).unwrap();
        assert_eq!(resp.total_results, 2);
        assert_eq!(resp.start_index, 0);
        assert_eq!(resp.items_per_page, 2);
        assert_eq!(resp.entries.len(), 2);
    }

    #[test]
    fn parse_first_entry() {
        let resp = parse_feed(SAMPLE_FEED).unwrap();
        let e = &resp.entries[0];

        assert_eq!(e.arxiv_id, "2401.12345v2");
        assert_eq!(
            e.title,
            "Attention Is All You Need: A Comprehensive Survey of Transformer Architectures"
        );
        assert!(
            e.summary
                .as_ref()
                .unwrap()
                .contains("transformer architectures")
        );
        assert_eq!(e.authors.len(), 2);
        assert_eq!(e.authors[0].name, "Ashish Vaswani");
        assert_eq!(e.authors[0].affiliation.as_deref(), Some("Google Brain"));
        assert_eq!(e.authors[1].name, "Noam Shazeer");
        assert!(e.authors[1].affiliation.is_none());
        assert_eq!(e.published.as_deref(), Some("2024-01-15T00:00:00Z"));
        assert_eq!(e.updated.as_deref(), Some("2024-01-20T12:00:00Z"));
        assert_eq!(e.primary_category.as_deref(), Some("cs.LG"));
        assert_eq!(e.categories, vec!["cs.LG", "cs.CL"]);
        assert_eq!(e.doi.as_deref(), Some("10.1000/test"));
        assert_eq!(e.journal_ref.as_deref(), Some("Nature 2024"));
        assert_eq!(e.comment.as_deref(), Some("15 pages, 3 figures"));
        assert!(e.pdf_url.as_ref().unwrap().contains("pdf/2401.12345v2"));
        assert!(e.abs_url.as_ref().unwrap().contains("abs/2401.12345v2"));
    }

    #[test]
    fn parse_second_entry() {
        let resp = parse_feed(SAMPLE_FEED).unwrap();
        let e = &resp.entries[1];

        assert_eq!(e.arxiv_id, "2309.01234v1");
        assert_eq!(e.title, "A Short Paper");
        assert_eq!(e.authors.len(), 1);
        assert_eq!(e.authors[0].name, "Jane Doe");
        assert_eq!(e.primary_category.as_deref(), Some("stat.ML"));
        assert!(e.doi.is_none());
        assert!(e.journal_ref.is_none());
    }

    #[test]
    fn atom_to_articles_conversion() {
        let resp = parse_feed(SAMPLE_FEED).unwrap();
        let articles = atom_to_articles(&resp.entries);
        assert_eq!(articles.len(), 2);

        let a = &articles[0];
        assert_eq!(a.identifier(IdKind::Arxiv), Some("2401.12345v2"));
        assert_eq!(a.doi(), Some("10.1000/test"));
        assert_eq!(
            a.title,
            "Attention Is All You Need: A Comprehensive Survey of Transformer Architectures"
        );
        assert_eq!(a.authors.len(), 2);
        assert_eq!(a.authors[0].last_name, "Vaswani");
        assert_eq!(a.authors[0].fore_name.as_deref(), Some("Ashish"));
        assert!(a.abstract_text.as_ref().unwrap().contains("transformer"));
        assert_eq!(a.year, Some(2024));
        assert_eq!(a.month, Some(1));
        assert_eq!(a.journal.as_deref(), Some("Nature 2024"));
        assert_eq!(a.source, ArticleSource::Arxiv);
        assert!(a.pub_types.contains(&"cs.LG".to_owned()));
        assert!(a.keywords.contains(&"cs.CL".to_owned()));
        assert!(a.keywords.iter().any(|k| k.contains("15 pages")));
    }

    #[test]
    fn extract_arxiv_id_from_url() {
        assert_eq!(
            extract_arxiv_id("http://arxiv.org/abs/2401.12345v2"),
            "2401.12345v2"
        );
        assert_eq!(
            extract_arxiv_id("https://arxiv.org/abs/cond-mat/0207270"),
            "cond-mat/0207270"
        );
        assert_eq!(extract_arxiv_id("bare-id"), "bare-id");
    }

    #[test]
    fn collapse_whitespace_works() {
        assert_eq!(collapse_whitespace("  hello   world  "), "hello world");
        assert_eq!(collapse_whitespace("\n\tmulti\n\tline\n"), "multi line");
    }

    #[test]
    fn parse_iso_date_basic() {
        assert_eq!(
            parse_iso_date("2024-01-15T00:00:00Z"),
            (Some(2024), Some(1))
        );
        assert_eq!(
            parse_iso_date("2023-12-31T23:59:59Z"),
            (Some(2023), Some(12))
        );
        assert_eq!(parse_iso_date("garbage"), (None, None));
    }

    #[test]
    fn parse_error_feed() {
        let error_xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<feed xmlns="http://www.w3.org/2005/Atom" xmlns:opensearch="http://a9.com/-/spec/opensearch/1.1/">
  <opensearch:totalResults xmlns:opensearch="http://a9.com/-/spec/opensearch/1.1/">0</opensearch:totalResults>
  <opensearch:startIndex xmlns:opensearch="http://a9.com/-/spec/opensearch/1.1/">0</opensearch:startIndex>
  <opensearch:itemsPerPage xmlns:opensearch="http://a9.com/-/spec/opensearch/1.1/">0</opensearch:itemsPerPage>
  <entry>
    <id>http://arxiv.org/api/errors#incorrect_id_format_for_some_id</id>
    <title>Error</title>
    <summary>Incorrect id format: some_id</summary>
  </entry>
</feed>"#;
        let resp = parse_feed(error_xml).unwrap();
        assert_eq!(resp.entries.len(), 1);
        assert!(resp.entries[0].arxiv_id.starts_with("api/errors"));
    }

    #[test]
    fn parse_author_name() {
        let a = parse_arxiv_author("Ashish Vaswani", Some("Google Brain"));
        assert_eq!(a.last_name, "Vaswani");
        assert_eq!(a.fore_name.as_deref(), Some("Ashish"));
        assert_eq!(a.affiliation.as_deref(), Some("Google Brain"));

        let b = parse_arxiv_author("Yann LeCun", None);
        assert_eq!(b.last_name, "LeCun");
        assert_eq!(b.fore_name.as_deref(), Some("Yann"));
        assert!(b.affiliation.is_none());

        // Single name
        let c = parse_arxiv_author("Galileo", None);
        assert_eq!(c.last_name, "Galileo");
        assert!(c.fore_name.is_none());
    }
}
