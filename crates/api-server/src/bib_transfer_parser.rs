//! Bounded, previewable interchange import. Unsupported fields are reported, not silently claimed migrated.
use bib_types::{Article, ArticleSource, Author, IdKind, Identifier};
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

#[derive(Serialize)]
pub(crate) struct ImportedRecord {
    pub article: Article,
    pub warnings: Vec<String>,
}
fn author(value: &str) -> Author {
    if let Some(literal) = value
        .trim()
        .strip_prefix('{')
        .and_then(|v| v.strip_suffix('}'))
    {
        return Author {
            last_name: literal.into(),
            fore_name: None,
            initials: None,
            affiliation: None,
            orcid: None,
            corresponding: false,
        };
    }
    let (last, first) = value
        .split_once(',')
        .map(|(a, b)| (a.trim(), Some(b.trim().to_owned())))
        .unwrap_or_else(|| {
            value
                .trim()
                .rsplit_once(' ')
                .map(|(a, b)| (b, Some(a.to_owned())))
                .unwrap_or((value.trim(), None))
        });
    Author {
        last_name: last.into(),
        fore_name: first,
        initials: None,
        affiliation: None,
        orcid: None,
        corresponding: false,
    }
}
fn identifier(article: &mut Article, kind: IdKind, value: Option<&str>) {
    if let Some(value) = value.filter(|v| !v.trim().is_empty()) {
        let value = value.trim();
        let value = if matches!(kind, IdKind::Doi | IdKind::Biorxiv) {
            value
                .trim_start_matches("https://doi.org/")
                .trim_start_matches("http://doi.org/")
                .trim_start_matches("doi:")
        } else {
            value
        };
        article.identifiers.push(Identifier::new(kind, value));
    }
}
fn finish(mut article: Article, warnings: Vec<String>) -> Result<ImportedRecord, String> {
    if article.title.trim().is_empty() {
        return Err("A record has no title; supply a title before importing".into());
    }
    article.title = article.title.trim().into();
    // Stable IDs make retries idempotent, including entries without identifiers.
    article.id = String::new();
    let digest = Sha256::digest(serde_json::to_vec(&article).map_err(|e| e.to_string())?);
    article.id = format!("import-{digest:x}");
    Ok(ImportedRecord { article, warnings })
}
fn item_type(kind: &str) -> String {
    match kind
        .to_ascii_lowercase()
        .replace([' ', '_', '-'], "")
        .as_str()
    {
        "book" => "Book",
        "booksection" | "incollection" | "chap" => "Book Section",
        "thesis" | "thes" | "phdthesis" | "mastersthesis" => "Thesis",
        "report" | "rprt" | "techreport" => "Report",
        "patent" | "pat" => "Patent",
        "webpage" | "web" | "elec" => "Web Page",
        "conferencepaper" | "inproceedings" | "cpaper" => "Conference Paper",
        "dataset" | "data" => "Dataset",
        "article" | "journalarticle" | "jour" => "Journal Article",
        _ => kind,
    }
    .to_owned()
}
fn from_fields(
    fields: BTreeMap<String, Vec<String>>,
    format: &str,
) -> Result<ImportedRecord, String> {
    let get = |keys: &[&str]| {
        keys.iter()
            .find_map(|key| fields.get(*key).and_then(|v| v.first()).cloned())
    };
    let mut a = Article::new("", get(&["title", "ti", "t1"]).unwrap_or_default());
    a.source = ArticleSource::Manual;
    a.journal = get(&["journal", "jo", "jf", "t2", "booktitle"]);
    a.abstract_text = get(&["abstract", "ab", "n2"]);
    a.year =
        get(&["year", "py", "y1", "date"]).and_then(|y| y.get(..4).and_then(|v| v.parse().ok()));
    a.volume = get(&["volume", "vl"]);
    a.issue = get(&["number", "is"]);
    a.pages = get(&["pages", "sp"]);
    if let (Some(start), Some(end)) = (a.pages.clone(), get(&["ep"])) {
        if !start.contains('-') {
            a.pages = Some(format!("{start}-{end}"));
        }
    }
    a.issn = get(&["issn", "sn"]);
    a.language = get(&["language", "la"]);
    if let Some(kind) = get(&["entrytype", "ty"]) {
        a.pub_types.push(item_type(&kind));
    }
    identifier(&mut a, IdKind::Doi, get(&["doi", "do"]).as_deref());
    identifier(&mut a, IdKind::Pmid, get(&["pmid"]).as_deref());
    identifier(&mut a, IdKind::Other, get(&["isbn"]).as_deref());
    if format == "ris" {
        a.authors = fields
            .get("au")
            .or(fields.get("a1"))
            .into_iter()
            .flatten()
            .map(|v| author(v))
            .collect();
        a.keywords = fields.get("kw").cloned().unwrap_or_default();
    } else {
        a.authors = get(&["author"])
            .map(|v| v.split(" and ").map(author).collect())
            .unwrap_or_default();
        a.keywords = get(&["keywords"])
            .map(|v| {
                v.split([',', ';'])
                    .map(|s| s.trim().into())
                    .filter(|s: &String| !s.is_empty())
                    .collect()
            })
            .unwrap_or_default();
    }
    let extra_mappings: &[(&str, &[&str])] = &[
        ("publisher", &["publisher", "pb"]),
        ("place", &["address", "place", "cy"]),
        ("edition", &["edition", "et"]),
        ("book_title", &["booktitle"]),
        ("university", &["school"]),
        ("institution", &["institution"]),
        ("report_type", &["type"]),
        ("patent_number", &["patentnumber"]),
        ("website_title", &["websitetitle"]),
        ("access_date", &["urldate", "y2"]),
        ("url", &["url", "ur"]),
    ];
    for (key, aliases) in extra_mappings {
        if let Some(value) = get(aliases) {
            a.extra_fields.insert((*key).into(), value);
        }
    }
    let known = [
        "title",
        "ti",
        "t1",
        "journal",
        "jo",
        "jf",
        "t2",
        "booktitle",
        "abstract",
        "ab",
        "n2",
        "year",
        "py",
        "y1",
        "date",
        "volume",
        "vl",
        "number",
        "is",
        "pages",
        "sp",
        "ep",
        "issn",
        "sn",
        "language",
        "la",
        "entrytype",
        "ty",
        "doi",
        "do",
        "pmid",
        "isbn",
        "au",
        "a1",
        "author",
        "keywords",
        "kw",
        "er",
    ];
    let omitted: Vec<_> = fields
        .keys()
        .filter(|k| {
            !known.contains(&k.as_str())
                && !extra_mappings
                    .iter()
                    .any(|(_, aliases)| aliases.contains(&k.as_str()))
        })
        .cloned()
        .collect();
    let mut warnings = Vec::new();
    if !omitted.is_empty() {
        warnings.push(format!("Fields not mapped: {}", omitted.join(", ")));
    }
    finish(a, warnings)
}
fn ris(contents: &str) -> Result<Vec<ImportedRecord>, String> {
    let mut records = Vec::new();
    let mut fields: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut previous = String::new();
    for (line_no, line) in contents.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let bytes = line.as_bytes();
        if bytes.len() >= 5
            && bytes[0].is_ascii_alphanumeric()
            && bytes[1].is_ascii_alphanumeric()
            && &bytes[2..5] == b"  -"
        {
            let key = line[..2].to_ascii_lowercase();
            let value = line[5..].trim();
            if key == "ty" && !fields.is_empty() {
                return Err(format!("RIS record missing ER before line {}", line_no + 1));
            }
            if fields.is_empty() && key != "ty" {
                return Err(format!(
                    "RIS record must start with TY (line {})",
                    line_no + 1
                ));
            }
            fields.entry(key.clone()).or_default().push(value.into());
            previous = key.clone();
            if key == "er" {
                records.push(from_fields(std::mem::take(&mut fields), "ris")?);
            }
        } else if let Some(value) = fields.get_mut(&previous).and_then(|v| v.last_mut()) {
            value.push('\n');
            value.push_str(line.trim());
        } else {
            return Err(format!("Invalid RIS line {}", line_no + 1));
        }
    }
    if !fields.is_empty() {
        return Err("RIS record missing closing ER tag".into());
    }
    Ok(records)
}
struct BibParser<'a> {
    text: &'a str,
    pos: usize,
}
impl<'a> BibParser<'a> {
    fn peek(&self) -> Option<char> {
        self.text[self.pos..].chars().next()
    }
    fn bump(&mut self) -> Option<char> {
        let c = self.peek()?;
        self.pos += c.len_utf8();
        Some(c)
    }
    fn ws(&mut self) {
        loop {
            while self.peek().is_some_and(char::is_whitespace) {
                self.bump();
            }
            if self.peek() == Some('%') {
                while self.bump().is_some_and(|c| c != '\n') {}
            } else {
                break;
            }
        }
    }
    fn take_until(&mut self, stops: &[char]) -> String {
        let start = self.pos;
        while self.peek().is_some_and(|c| !stops.contains(&c)) {
            self.bump();
        }
        self.text[start..self.pos].trim().to_owned()
    }
    fn value(&mut self) -> Result<String, String> {
        self.ws();
        let open = self.peek();
        if open == Some('{') || open == Some('"') {
            self.bump();
            let close = if open == Some('{') { '}' } else { '"' };
            let mut depth = 0;
            let mut out = String::new();
            while let Some(c) = self.bump() {
                if c == '\\' {
                    out.push(c);
                    if let Some(next) = self.bump() {
                        out.push(next);
                    }
                    continue;
                }
                if c == close && depth == 0 {
                    return Ok(out);
                }
                if c == '{' {
                    depth += 1;
                }
                if c == '}' {
                    if depth == 0 {
                        return Err("Unbalanced BibTeX value".into());
                    }
                    depth -= 1;
                }
                out.push(c);
            }
            Err("Unclosed BibTeX value".into())
        } else {
            let value = self.take_until(&[',', '}', ')', '#']);
            if value.chars().all(|c| c.is_ascii_digit()) && !value.is_empty() {
                Ok(value)
            } else {
                Err(format!(
                    "Unsupported BibTeX macro {value:?}; export with strings expanded"
                ))
            }
        }
    }
}
fn bibtex(contents: &str) -> Result<Vec<ImportedRecord>, String> {
    let mut p = BibParser {
        text: contents,
        pos: 0,
    };
    let mut records = Vec::new();
    loop {
        p.ws();
        if p.peek().is_none() {
            break;
        }
        if p.bump() != Some('@') {
            return Err(format!("Expected BibTeX @entry at byte {}", p.pos));
        }
        let kind = p.take_until(&['{', '(']).to_ascii_lowercase();
        let open = p.bump().ok_or("Missing BibTeX entry body")?;
        let close = if open == '{' { '}' } else { ')' };
        if ["string", "preamble", "comment"].contains(&kind.as_str()) {
            return Err(format!(
                "BibTeX @{kind} is not supported; export with strings expanded and comments removed"
            ));
        }
        let _key = p.take_until(&[',', close]);
        if p.bump() != Some(',') {
            return Err("BibTeX entry has no fields".into());
        }
        let mut fields = BTreeMap::new();
        fields.insert("entrytype".into(), vec![kind]);
        loop {
            p.ws();
            if p.peek() == Some(close) {
                p.bump();
                break;
            }
            let key = p.take_until(&['=', ',', close]).to_ascii_lowercase();
            if p.bump() != Some('=') {
                return Err(format!("Expected '=' after field {key}"));
            }
            let value = p.value()?;
            fields.entry(key).or_insert_with(Vec::new).push(value);
            p.ws();
            match p.peek() {
                Some(',') => {
                    p.bump();
                }
                Some(c) if c == close => {}
                _ => return Err("Expected comma or closing delimiter in BibTeX".into()),
            }
        }
        let mut record = from_fields(fields, "bibtex")?;
        if record.article.title.contains('\\') || record.article.title.contains('{') {
            record
                .warnings
                .push("TeX markup is retained; review title and author spelling".into());
        }
        records.push(record);
    }
    Ok(records)
}
fn zotero(contents: &str) -> Result<Vec<ImportedRecord>, String> {
    let value: Value = serde_json::from_str(contents).map_err(|e| e.to_string())?;
    let items = value
        .as_array()
        .or_else(|| value.get("items").and_then(Value::as_array))
        .ok_or("Expected a Zotero JSON array or object with items")?;
    items.iter().map(|item|item.get("data").unwrap_or(item)).filter(|item|!matches!(item.get("itemType").and_then(Value::as_str),Some("note"|"attachment"|"annotation"))).map(|item|{
        let item=item.get("data").unwrap_or(item);let string=|key:&str|item.get(key).and_then(Value::as_str).map(str::to_owned);
        let mut a=Article::new("",string("title").unwrap_or_default());a.source=ArticleSource::Zotero;
        a.journal=string("publicationTitle").or_else(||string("bookTitle"));a.abstract_text=string("abstractNote");a.volume=string("volume");a.issue=string("issue");a.pages=string("pages");a.issn=string("ISSN");a.language=string("language");
        a.year=string("date").and_then(|s|s.split(|c:char|!c.is_ascii_digit()).find(|s|s.len()==4).and_then(|s|s.parse().ok()));
        if let Some(kind)=string("itemType"){a.pub_types.push(item_type(&kind));}
        identifier(&mut a,IdKind::Doi,string("DOI").as_deref());identifier(&mut a,IdKind::Other,string("ISBN").as_deref());
        for (key,source) in [("publisher","publisher"),("place","place"),("edition","edition"),("book_title","bookTitle"),("university","university"),("institution","institution"),("report_type","reportType"),("patent_number","patentNumber"),("website_title","websiteTitle"),("access_date","accessDate"),("url","url")] { if let Some(value)=string(source) { a.extra_fields.insert(key.into(),value); } }
        let mut warnings=Vec::new();
        if let Some(creators)=item.get("creators").and_then(Value::as_array){for creator in creators {if creator.get("creatorType").and_then(Value::as_str).is_some_and(|t|t!="author"){warnings.push("Non-author contributor roles are not mapped".into());continue;}a.authors.push(Author{last_name:creator.get("lastName").or_else(||creator.get("name")).and_then(Value::as_str).unwrap_or("").into(),fore_name:creator.get("firstName").and_then(Value::as_str).map(str::to_owned),initials:None,affiliation:None,orcid:None,corresponding:false});}}
        a.keywords=item.get("tags").and_then(Value::as_array).into_iter().flatten().filter_map(|tag|tag.as_str().or_else(||tag.get("tag").and_then(Value::as_str))).map(str::to_owned).collect();
        warnings.push("Standard import transfers metadata only. Use the Zotero bundle importer for supplied files, notes and collections. Document citation links are not migrated".into());
        finish(a,warnings)
    }).collect()
}
pub(crate) fn parse(format: &str, contents: &str) -> Result<Vec<ImportedRecord>, String> {
    if contents.len() > 8 * 1024 * 1024 {
        return Err("Import exceeds 8 MiB".into());
    }
    let records = match format {
        "bibtex" => bibtex(contents),
        "ris" => ris(contents),
        "zotero_json" => zotero(contents),
        _ => Err("format must be bibtex, ris, or zotero_json".into()),
    }?;
    if records.is_empty() {
        return Err("No importable records found".into());
    }
    if records.len() > 2000 {
        return Err("Import exceeds 2000 records; split the file".into());
    }
    Ok(records)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn nested_unicode_bibtex() {
        let r=parse("bibtex","@article{a, title={A {nested} 中文 title}, author={Wang, Li and Smith, Jane}, year=2024, doi={10.1234/a}, journal={J}} ").unwrap();
        assert_eq!(r[0].article.authors[0].last_name, "Wang");
        assert_eq!(r[0].article.year, Some(2024));
        assert_eq!(r[0].article.id,parse("bibtex","@article{a, title={A {nested} 中文 title}, author={Wang, Li and Smith, Jane}, year=2024, doi={10.1234/a}, journal={J}} ").unwrap()[0].article.id);
    }
    #[test]
    fn ris_continuations_and_pages() {
        let r=parse("ris","TY  - JOUR\nTI  - Study\nAU  - Smith, Jane\nSP  - 1\nEP  - 9\nAB  - First\n second\nER  -\n").unwrap();
        assert_eq!(r[0].article.pages.as_deref(), Some("1-9"));
        assert_eq!(r[0].article.abstract_text.as_deref(), Some("First\nsecond"));
        assert!(parse("ris", "TY  - JOUR\nTI  - bad").is_err());
    }
    #[test]
    fn zotero_preserves_tags_and_warns() {
        let r=parse("zotero_json",r#"{"items":[{"itemType":"journalArticle","title":"Study","DOI":"10.1/a","tags":[{"tag":"中文"}],"creators":[{"creatorType":"author","lastName":"Li","firstName":"Ming"}]}]}"#).unwrap();
        assert_eq!(r[0].article.keywords, vec!["中文"]);
        assert!(!r[0].warnings.is_empty());
    }
    #[test]
    fn reject_malformed_and_macros() {
        assert!(parse("bibtex", "@article{x,title={no close}").is_err());
        assert!(parse("bibtex", "@article{x,title=macro}").is_err());
    }
}
