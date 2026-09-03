//! Turn bibliography files (BibTeX / RIS / CSL-JSON) into [`Article`] rows.
//!
//! The three parsers share one intermediate shape so field mapping lives in a
//! single place; format quirks stay in their own functions. Entries that lack
//! a title are reported as failures instead of silently dropped, so the API
//! can show the user what did not make it in.

use bib_types::{Article, ArticleSource, Author, IdKind, Identifier};
use biblatex::{DateValue, PermissiveType};
use chrono::Utc;

/// Supported bibliography file formats.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImportFormat {
    Bibtex,
    Ris,
    CslJson,
}

impl ImportFormat {
    /// Resolve the client-supplied format name (`"auto"` excluded).
    pub fn from_name(name: &str) -> Option<Self> {
        match name.to_ascii_lowercase().as_str() {
            "bibtex" | "bib" => Some(Self::Bibtex),
            "ris" => Some(Self::Ris),
            "csl_json" | "csl-json" | "csljson" => Some(Self::CslJson),
            _ => None,
        }
    }

    /// Guess the format from the content itself, used by `auto` imports.
    pub fn sniff(content: &str) -> Option<Self> {
        let trimmed = content.trim_start();
        if trimmed.starts_with('[') || trimmed.starts_with('{') {
            return Some(Self::CslJson);
        }
        if trimmed.starts_with('@') {
            return Some(Self::Bibtex);
        }
        // RIS records open with a `TY  - ` line.
        if content
            .lines()
            .any(|line| ris_line(line).is_some_and(|(tag, _)| tag == "TY"))
        {
            return Some(Self::Ris);
        }
        None
    }
}

/// An entry that could not be turned into an article.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportFailure {
    /// Citation key, RIS record number, or CSL id — whatever identifies the
    /// entry inside its file.
    pub key: String,
    pub reason: String,
}

/// Parse `content` as the given format.
pub fn parse_import(format: ImportFormat, content: &str) -> (Vec<Article>, Vec<ImportFailure>) {
    match format {
        ImportFormat::Bibtex => parse_bibtex(content),
        ImportFormat::Ris => parse_ris(content),
        ImportFormat::CslJson => parse_csl_json(content),
    }
}

/// Format-neutral entry produced by every parser.
struct RawEntry {
    key: String,
    title: Option<String>,
    authors: Vec<(String, String)>,
    year: Option<i32>,
    month: Option<u32>,
    journal: Option<String>,
    volume: Option<String>,
    issue: Option<String>,
    pages: Option<String>,
    doi: Option<String>,
    arxiv: Option<String>,
    pmid: Option<String>,
    abstract_text: Option<String>,
    keywords: Vec<String>,
}

impl RawEntry {
    fn new(key: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            title: None,
            authors: Vec::new(),
            year: None,
            month: None,
            journal: None,
            volume: None,
            issue: None,
            pages: None,
            doi: None,
            arxiv: None,
            pmid: None,
            abstract_text: None,
            keywords: Vec::new(),
        }
    }
}

/// Build an article, or explain why the entry is unusable.
///
/// The id prefers a real identifier (`doi:…` / `arxiv:…` / `pmid:…`) so
/// re-importing the same record from another file deduplicates naturally;
/// entries without any identifier fall back to the citation key under a
/// format namespace.
fn raw_to_article(raw: RawEntry, source_key_prefix: &str) -> Result<Article, String> {
    let title = raw
        .title
        .as_deref()
        .map(str::trim)
        .filter(|title| !title.is_empty())
        .ok_or_else(|| "entry has no title".to_owned())?
        .to_owned();

    let mut identifiers = Vec::new();
    if let Some(doi) = clean(raw.doi.as_deref()) {
        identifiers.push(Identifier::doi(doi));
    }
    if let Some(pmid) = clean(raw.pmid.as_deref()) {
        identifiers.push(Identifier::pmid(pmid));
    }
    if let Some(arxiv) = clean(raw.arxiv.as_deref()) {
        identifiers.push(Identifier::new(IdKind::Arxiv, arxiv));
    }

    let primary = identifiers.first();
    let id = match primary {
        Some(identifier) => format!("{}:{}", identifier.kind.as_str(), identifier.value),
        None => format!("{source_key_prefix}:{}", raw.key),
    };

    let authors = raw
        .authors
        .into_iter()
        .filter(|(last, _)| !last.trim().is_empty())
        .map(|(last, fore)| Author {
            last_name: last,
            fore_name: (!fore.trim().is_empty()).then_some(fore),
            initials: None,
            affiliation: None,
            orcid: None,
            corresponding: false,
        })
        .collect();

    let now = Utc::now();
    Ok(Article {
        id,
        title,
        authors,
        identifiers,
        abstract_text: clean(raw.abstract_text.as_deref()),
        year: raw
            .year
            .filter(|year| (1000..=3000).contains(year))
            .map(|year| year as u16),
        month: raw
            .month
            .filter(|month| (1..=12).contains(month))
            .map(|month| month as u8),
        journal: clean(raw.journal.as_deref()),
        volume: clean(raw.volume.as_deref()),
        issue: clean(raw.issue.as_deref()),
        pages: clean(raw.pages.as_deref()),
        issn: None,
        essn: None,
        language: None,
        pub_types: Vec::new(),
        keywords: raw.keywords,
        source: ArticleSource::Manual,
        created_at: Some(now),
        updated_at: Some(now),
    })
}

fn clean(value: Option<&str>) -> Option<String> {
    let value = value?.trim();
    (!value.is_empty()).then(|| value.to_owned())
}

// --- BibTeX ---------------------------------------------------------------

fn parse_bibtex(content: &str) -> (Vec<Article>, Vec<ImportFailure>) {
    let bibliography = match biblatex::Bibliography::parse(content) {
        Ok(bibliography) => bibliography,
        Err(parse_error) => {
            return (
                Vec::new(),
                vec![ImportFailure {
                    key: "<file>".to_owned(),
                    reason: format!("BibTeX parse error: {parse_error}"),
                }],
            )
        }
    };

    let mut articles = Vec::new();
    let mut failures = Vec::new();
    for entry in bibliography {
        let key = entry.key.clone();
        let mut raw = RawEntry::new(key.clone());

        raw.title = string_field(&entry, "title");
        raw.journal =
            string_field(&entry, "journal").or_else(|| string_field(&entry, "journaltitle"));
        raw.issue = string_field(&entry, "number");
        raw.abstract_text =
            string_field(&entry, "abstract").or_else(|| string_field(&entry, "annote"));
        if let Some(keywords) = string_field(&entry, "keywords") {
            raw.keywords = split_keywords(&keywords);
        }

        if let Ok(authors) = entry.author() {
            raw.authors = authors
                .into_iter()
                .map(|person| {
                    let mut last = person.name;
                    if !person.prefix.is_empty() {
                        last = format!("{} {last}", person.prefix);
                    }
                    (last, person.given_name)
                })
                .collect();
        }

        if let Ok(PermissiveType::Typed(date)) = entry.date() {
            let datetime = match date.value {
                DateValue::At(at) | DateValue::After(at) | DateValue::Before(at) => Some(at),
                DateValue::Between(start, _) => Some(start),
            };
            if let Some(at) = datetime {
                raw.year = Some(at.year);
                raw.month = at.month.map(u32::from);
            }
        }

        if let Ok(PermissiveType::Typed(volume)) = entry.volume() {
            raw.volume = Some(volume.to_string());
        }
        if let Ok(PermissiveType::Typed(pages)) = entry.pages() {
            raw.pages = Some(
                pages
                    .iter()
                    .map(|range| {
                        // biblatex ranges are inclusive (`100--110` → 100..=110);
                        // a lone page arrives as a degenerate range.
                        if range.end > range.start {
                            format!("{}-{}", range.start, range.end)
                        } else {
                            range.start.to_string()
                        }
                    })
                    .collect::<Vec<_>>()
                    .join(","),
            );
        }

        if let Ok(doi) = entry.doi() {
            raw.doi = Some(doi);
        }
        if let Ok(eprint) = entry.eprint() {
            // `eprint` carries the arXiv id when `eprinttype`/`archiveprefix`
            // says so; the bare id pattern is distinctive enough to accept on
            // its own for Zotero exports that drop the type field.
            let eprint_type = string_field(&entry, "eprinttype")
                .or_else(|| string_field(&entry, "archiveprefix"))
                .map(|kind| kind.to_lowercase())
                .unwrap_or_default();
            if eprint_type.contains("arxiv") || looks_like_arxiv_id(&eprint) {
                raw.arxiv = Some(eprint);
            }
        }

        match raw_to_article(raw, "bibtex") {
            Ok(article) => articles.push(article),
            Err(reason) => failures.push(ImportFailure { key, reason }),
        }
    }
    (articles, failures)
}

/// Read a plain-string field, tolerating values biblatex cannot fully type.
fn string_field(entry: &biblatex::Entry, field: &str) -> Option<String> {
    entry
        .get_as::<String>(field)
        .ok()
        .and_then(|value| clean(Some(&value)))
}

/// Recognize an arXiv id in either generation: `2401.12345[v2]` or
/// `hep-ph/9901001`.
fn looks_like_arxiv_id(value: &str) -> bool {
    let value = value.trim();
    let core = match value.split_once('v') {
        // Only strip a version suffix that follows the id's last digit.
        Some((head, tail))
            if tail.chars().all(|c| c.is_ascii_digit())
                && head.ends_with(|c: char| c.is_ascii_digit()) =>
        {
            head
        }
        _ => value,
    };

    if let Some((year_month, sequence)) = core.split_once('.') {
        return year_month.len() == 4
            && year_month.bytes().all(|b| b.is_ascii_digit())
            && (4..=5).contains(&sequence.len())
            && sequence.bytes().all(|b| b.is_ascii_digit());
    }
    if let Some((category, number)) = core.split_once('/') {
        return category.bytes().all(|b| b.is_ascii_lowercase() || b == b'-')
            && number.len() == 7
            && number.bytes().all(|b| b.is_ascii_digit());
    }
    false
}

fn split_keywords(raw: &str) -> Vec<String> {
    raw.split([',', ';'])
        .map(str::trim)
        .filter(|keyword| !keyword.is_empty())
        .map(str::to_owned)
        .collect()
}

// --- RIS ------------------------------------------------------------------

/// Parse RIS: `XX  - value` lines, `ER` ends a record, untagged lines
/// continue the previous value.
fn parse_ris(content: &str) -> (Vec<Article>, Vec<ImportFailure>) {
    let mut articles = Vec::new();
    let mut failures = Vec::new();

    let mut raw: Option<RawEntry> = None;
    let mut last_tag: Option<String> = None;

    let flush = |raw: Option<RawEntry>,
                     articles: &mut Vec<Article>,
                     failures: &mut Vec<ImportFailure>| {
        if let Some(entry) = raw {
            match raw_to_article(entry, "ris") {
                Ok(article) => articles.push(article),
                Err(reason) => failures.push(ImportFailure {
                    key: format!("ris-{}", articles.len() + failures.len() + 1),
                    reason,
                }),
            }
        }
    };

    for line in content.lines() {
        if let Some((tag, value)) = ris_line(line) {
            if tag == "ER" {
                flush(raw.take(), &mut articles, &mut failures);
                last_tag = None;
                continue;
            }
            let entry = raw.get_or_insert_with(|| RawEntry::new(String::new()));
            if entry.key.is_empty() {
                entry.key = format!("ris-{}", articles.len() + failures.len() + 1);
            }
            apply_ris_tag(entry, &tag, &value);
            last_tag = Some(tag);
        } else if !line.trim().is_empty() {
            // Continuation line: append to whichever tag we last saw.
            if let (Some(entry), Some(tag)) = (raw.as_mut(), last_tag.as_deref()) {
                append_ris_continuation(entry, tag, line.trim());
            }
        }
    }
    // A file whose last record lacks `ER` still deserves an import attempt.
    flush(raw, &mut articles, &mut failures);
    (articles, failures)
}

/// Split `XX  - value`: a two-letter tag, whitespace, `-`, then the value.
fn ris_line(line: &str) -> Option<(String, String)> {
    let bytes = line.as_bytes();
    if bytes.len() < 4 || !bytes[0].is_ascii_alphabetic() || !bytes[1].is_ascii_alphabetic() {
        return None;
    }
    let after_tag = line[2..].trim_start();
    let value = after_tag.strip_prefix('-')?;
    Some((
        line[..2].to_ascii_uppercase(),
        value.trim_start().trim().to_owned(),
    ))
}

fn apply_ris_tag(entry: &mut RawEntry, tag: &str, value: &str) {
    match tag {
        // Repeat tags continue the field (multi-line titles), matching how
        // Zotero exports wrapped values.
        "TI" | "T1" | "CT" => append(&mut entry.title, value),
        "AU" | "A1" => entry.authors.push(parse_ris_author(value)),
        "PY" | "Y1" => {
            entry.year = value
                .chars()
                .take_while(|c| c.is_ascii_digit())
                .collect::<String>()
                .parse()
                .ok();
        }
        "JO" | "JF" | "T2" => append(&mut entry.journal, value),
        "VL" => entry.volume = Some(value.to_owned()),
        "IS" => entry.issue = Some(value.to_owned()),
        "SP" => {
            let start = value.to_owned();
            entry.pages = Some(match &entry.pages {
                // Combine with a previously seen `EP` into `start-end`.
                Some(existing) if existing.starts_with('-') => format!("{start}{existing}"),
                _ => start,
            });
        }
        "EP" => {
            let end = value.to_owned();
            entry.pages = Some(match &entry.pages {
                Some(existing) if !existing.contains('-') => format!("{existing}-{end}"),
                _ => format!("-{end}"),
            });
        }
        "DO" => entry.doi = Some(value.to_owned()),
        "AB" | "N2" => append(&mut entry.abstract_text, value),
        "KW" => entry.keywords.push(value.to_owned()),
        _ => {}
    }
}

fn append_ris_continuation(entry: &mut RawEntry, tag: &str, value: &str) {
    match tag {
        "TI" | "T1" | "CT" => append(&mut entry.title, value),
        "JO" | "JF" | "T2" => append(&mut entry.journal, value),
        "AB" | "N2" => append(&mut entry.abstract_text, value),
        _ => {}
    }
}

fn append(slot: &mut Option<String>, value: &str) {
    match slot {
        Some(existing) => {
            existing.push(' ');
            existing.push_str(value);
        }
        None => *slot = Some(value.to_owned()),
    }
}

/// RIS authors are `Family, Given` or `Given Family`.
fn parse_ris_author(value: &str) -> (String, String) {
    let value = value.trim();
    if let Some((last, fore)) = value.split_once(',') {
        return (last.trim().to_owned(), fore.trim().to_owned());
    }
    match value.rsplit_once(' ') {
        Some((fore, last)) => (last.to_owned(), fore.to_owned()),
        None => (value.to_owned(), String::new()),
    }
}

// --- CSL-JSON ---------------------------------------------------------------

fn parse_csl_json(content: &str) -> (Vec<Article>, Vec<ImportFailure>) {
    let parsed: serde_json::Value = match serde_json::from_str(content) {
        Ok(value) => value,
        Err(error) => {
            return (
                Vec::new(),
                vec![ImportFailure {
                    key: "<file>".to_owned(),
                    reason: format!("invalid JSON: {error}"),
                }],
            )
        }
    };
    let items = match parsed {
        serde_json::Value::Array(items) => items,
        serde_json::Value::Object(_) => vec![parsed],
        _ => {
            return (
                Vec::new(),
                vec![ImportFailure {
                    key: "<file>".to_owned(),
                    reason: "CSL-JSON must be an array of items".to_owned(),
                }],
            )
        }
    };

    let mut articles = Vec::new();
    let mut failures = Vec::new();
    for item in items {
        let key = item
            .get("id")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("unknown")
            .to_owned();
        match csl_item_to_raw(&item) {
            Some(raw) => match raw_to_article(raw, "csl") {
                Ok(article) => articles.push(article),
                Err(reason) => failures.push(ImportFailure { key, reason }),
            },
            None => failures.push(ImportFailure {
                key,
                reason: "item is not an object".to_owned(),
            }),
        }
    }
    (articles, failures)
}

fn csl_item_to_raw(item: &serde_json::Value) -> Option<RawEntry> {
    let object = item.as_object()?;
    let mut raw = RawEntry::new(
        object
            .get("id")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_owned(),
    );

    raw.title = csl_string(object.get("title"));
    raw.journal = csl_string(object.get("container-title"));
    raw.volume = csl_string(object.get("volume"));
    raw.issue = csl_string(object.get("issue"));
    raw.pages = csl_string(object.get("page"));
    raw.doi = csl_string(object.get("DOI")).or_else(|| csl_string(object.get("doi")));
    raw.pmid = csl_string(object.get("PMID")).or_else(|| csl_string(object.get("pmid")));
    raw.abstract_text = csl_string(object.get("abstract"));

    if let Some(keywords) = object.get("keyword").and_then(serde_json::Value::as_str) {
        raw.keywords = split_keywords(keywords);
    }

    if let Some(authors) = object.get("author").and_then(serde_json::Value::as_array) {
        raw.authors = authors
            .iter()
            .filter_map(|author| {
                if let Some(literal) = author.get("literal").and_then(serde_json::Value::as_str) {
                    return Some(parse_ris_author(literal));
                }
                let family = author.get("family").and_then(serde_json::Value::as_str)?;
                let given = author
                    .get("given")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default();
                Some((family.to_owned(), given.to_owned()))
            })
            .collect();
    }

    if let Some(issued) = object.get("issued") {
        // `{"date-parts": [[year, month?]]}`; tolerate a bare number too.
        if let Some(parts) = issued
            .get("date-parts")
            .and_then(serde_json::Value::as_array)
            .and_then(|lists| lists.first())
            .and_then(serde_json::Value::as_array)
        {
            raw.year = parts
                .first()
                .and_then(serde_json::Value::as_i64)
                .map(|year| year as i32);
            raw.month = parts
                .get(1)
                .and_then(serde_json::Value::as_i64)
                .map(|month| month as u32);
        } else if let Some(year) = issued.as_i64() {
            raw.year = Some(year as i32);
        }
    }

    Some(raw)
}

fn csl_string(value: Option<&serde_json::Value>) -> Option<String> {
    match value? {
        serde_json::Value::String(text) => clean(Some(text)),
        // Some producers wrap titles/containers in single-element arrays.
        serde_json::Value::Array(items) => items
            .first()
            .and_then(serde_json::Value::as_str)
            .and_then(|text| clean(Some(text))),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bibtex_maps_fields_and_dedupable_ids() {
        let source = r#"
@article{smith2024,
  author = {von Neumann, John and Ada Lovelace},
  title = {Off-target detection at scale},
  journal = {Nature Methods},
  year = {2024},
  volume = {21},
  number = {3},
  pages = {100--110},
  doi = {10.1000/off-target},
  abstract = {A study.},
  keywords = {crispr, screening}
}
"#;
        let (articles, failures) = parse_import(ImportFormat::Bibtex, source);
        assert!(failures.is_empty());
        let article = &articles[0];
        assert_eq!(article.id, "doi:10.1000/off-target");
        assert_eq!(article.title, "Off-target detection at scale");
        assert_eq!(article.authors[0].last_name, "von Neumann");
        assert_eq!(article.authors[0].fore_name.as_deref(), Some("John"));
        assert_eq!(article.authors[1].last_name, "Lovelace");
        assert_eq!(article.year, Some(2024));
        assert_eq!(article.journal.as_deref(), Some("Nature Methods"));
        assert_eq!(article.volume.as_deref(), Some("21"));
        assert_eq!(article.pages.as_deref(), Some("100-110"));
        assert_eq!(article.doi(), Some("10.1000/off-target"));
        assert_eq!(article.keywords, vec!["crispr", "screening"]);
    }

    #[test]
    fn bibtex_without_identifier_falls_back_to_citation_key() {
        let source = "@misc{note1, title = {A note}}";
        let (articles, failures) = parse_import(ImportFormat::Bibtex, source);
        assert!(failures.is_empty());
        assert_eq!(articles[0].id, "bibtex:note1");
    }

    #[test]
    fn bibtex_entries_without_titles_fail_with_reasons() {
        let source = "@article{empty1, author = {Nobody}}\n@article{good1, title = {Kept}}";
        let (articles, failures) = parse_import(ImportFormat::Bibtex, source);
        assert_eq!(articles.len(), 1);
        assert_eq!(failures.len(), 1);
        assert_eq!(failures[0].key, "empty1");
        assert!(failures[0].reason.contains("title"));
    }

    #[test]
    fn bibtex_arxiv_eprint_is_recognized() {
        let source =
            "@article{preprint2024, title = {Widgets}, eprint = {2401.12345}, archiveprefix = {arXiv}}";
        let (articles, failures) = parse_import(ImportFormat::Bibtex, source);
        assert!(failures.is_empty());
        assert_eq!(articles[0].id, "arxiv:2401.12345");
    }

    #[test]
    fn arxiv_id_patterns() {
        assert!(looks_like_arxiv_id("2401.12345"));
        assert!(looks_like_arxiv_id("2401.12345v2"));
        assert!(looks_like_arxiv_id("hep-ph/9901001"));
        assert!(!looks_like_arxiv_id("10.1000/off-target"));
        assert!(!looks_like_arxiv_id("12345"));
        assert!(!looks_like_arxiv_id("2401.12"));
    }

    #[test]
    fn ris_records_map_fields_and_split_authors() {
        let source = "\
TY  - JOUR
AU  - Smith, John A.
AU  - Jones B
TI  - A RIS record with a very long title
TI  - continued on the next line
JO  - J Clin Oncol
PY  - 2023
VL  - 12
IS  - 2
SP  - 33
EP  - 45
DO  - 10.2000/ris-test
AB  - An abstract.
KW  - oncology
ER  -
";
        let (articles, failures) = parse_import(ImportFormat::Ris, source);
        assert!(failures.is_empty(), "{failures:?}");
        let article = &articles[0];
        assert_eq!(article.id, "doi:10.2000/ris-test");
        assert_eq!(
            article.title,
            "A RIS record with a very long title continued on the next line"
        );
        assert_eq!(article.authors[0].last_name, "Smith");
        assert_eq!(article.authors[0].fore_name.as_deref(), Some("John A."));
        assert_eq!(article.authors[1].last_name, "B");
        assert_eq!(article.year, Some(2023));
        assert_eq!(article.pages.as_deref(), Some("33-45"));
        assert_eq!(article.keywords, vec!["oncology"]);
    }

    #[test]
    fn ris_without_identifiers_gets_indexed_ids() {
        let source = "TY  - JOUR\nTI  - One\nER  - \nTY  - JOUR\nTI  - Two\nER  - \n";
        let (articles, failures) = parse_import(ImportFormat::Ris, source);
        assert!(failures.is_empty());
        assert_eq!(articles[0].id, "ris:ris-1");
        assert_eq!(articles[1].id, "ris:ris-2");
    }

    #[test]
    fn csl_json_round_trips_the_export_shape() {
        let source = r#"[
            {
                "id": "doi:10.3000/csl",
                "type": "article-journal",
                "title": "A CSL item",
                "author": [{"family": "Doe", "given": "Jane"}],
                "issued": {"date-parts": [[2022, 5]]},
                "container-title": "Some Journal",
                "volume": "7",
                "issue": "1",
                "page": "1-9",
                "DOI": "10.3000/csl",
                "abstract": "Abstract text."
            }
        ]"#;
        let (articles, failures) = parse_import(ImportFormat::CslJson, source);
        assert!(failures.is_empty());
        let article = &articles[0];
        assert_eq!(article.id, "doi:10.3000/csl");
        assert_eq!(article.title, "A CSL item");
        assert_eq!(article.authors[0].last_name, "Doe");
        assert_eq!(article.year, Some(2022));
        assert_eq!(article.month, Some(5));
        assert_eq!(article.journal.as_deref(), Some("Some Journal"));
    }

    #[test]
    fn sniff_detects_all_three_formats() {
        assert_eq!(
            ImportFormat::sniff("@article{a, title={T}}"),
            Some(ImportFormat::Bibtex)
        );
        assert_eq!(
            ImportFormat::sniff("TY  - JOUR\nTI  - X\nER  - "),
            Some(ImportFormat::Ris)
        );
        assert_eq!(
            ImportFormat::sniff("[{\"id\":\"a\"}]"),
            Some(ImportFormat::CslJson)
        );
        assert_eq!(ImportFormat::sniff("just some text"), None);
    }

    #[test]
    fn broken_files_report_a_single_failure() {
        let (articles, failures) = parse_import(ImportFormat::Bibtex, "@article{unclosed, title={");
        assert!(articles.is_empty());
        assert_eq!(failures.len(), 1);
        assert_eq!(failures[0].key, "<file>");

        let (articles, failures) = parse_import(ImportFormat::CslJson, "{not json");
        assert!(articles.is_empty());
        assert_eq!(failures.len(), 1);
    }
}
