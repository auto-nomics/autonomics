//! Bibliographic export — render [`Article`] into standard citation formats.
//!
//! Currently supports BibTeX, RIS, and a compact Markdown citation.
//! CSL-JSON can be produced via `serde_json::to_value` since [`Article`]
//! already derives `Serialize`.

use bib_types::{Article, ExportFormat};

/// Render a single article into the requested citation format.
pub fn render(article: &Article, format: ExportFormat) -> String {
    match format {
        ExportFormat::Bibtex => to_bibtex(article),
        ExportFormat::Ris => to_ris(article),
        ExportFormat::Markdown => to_markdown(article),
        ExportFormat::CslJson => {
            serde_json::to_string_pretty(&to_csl_json(article)).unwrap_or_default()
        }
    }
}

/// Render a collection of articles as a single export string.
pub fn render_all(articles: &[Article], format: ExportFormat) -> String {
    if matches!(format, ExportFormat::CslJson) {
        return serde_json::to_string_pretty(&articles.iter().map(to_csl_json).collect::<Vec<_>>())
            .unwrap_or_else(|_| "[]".into());
    }
    articles
        .iter()
        .map(|a| render(a, format))
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn article_isbn(article: &Article) -> Option<&str> {
    article
        .identifiers
        .iter()
        .find(|id| {
            matches!(id.kind, bib_types::IdKind::Other)
                && id
                    .value
                    .chars()
                    .all(|c| c.is_ascii_digit() || matches!(c, 'X' | 'x' | '-' | ' '))
                && matches!(
                    id.value
                        .chars()
                        .filter(|c| c.is_ascii_alphanumeric())
                        .count(),
                    10 | 13
                )
        })
        .map(|id| id.value.as_str())
}
fn article_url(article: &Article) -> Option<&str> {
    if let Some(url) = article
        .extra_fields
        .get("url")
        .filter(|url| url.starts_with("https://") || url.starts_with("http://"))
    {
        return Some(url);
    }
    article
        .identifiers
        .iter()
        .find(|id| {
            matches!(id.kind, bib_types::IdKind::Other)
                && (id.value.starts_with("https://") || id.value.starts_with("http://"))
        })
        .map(|id| id.value.as_str())
}
fn entry_type(article: &Article) -> (&'static str, &'static str, &'static str) {
    let kind = article
        .pub_types
        .first()
        .map(|v| v.to_ascii_lowercase().replace([' ', '_', '-'], ""))
        .unwrap_or_default();
    match kind.as_str() {
        "book" => ("book", "BOOK", "book"),
        "booksection" | "incollection" | "chap" => ("incollection", "CHAP", "chapter"),
        "thesis" | "phdthesis" | "mastersthesis" | "thes" => ("phdthesis", "THES", "thesis"),
        "report" | "techreport" | "rprt" => ("techreport", "RPRT", "report"),
        "patent" | "pat" => ("misc", "PAT", "patent"),
        "webpage" | "web" | "elec" => ("misc", "ELEC", "webpage"),
        "conferencepaper" | "inproceedings" | "cpaper" => {
            ("inproceedings", "CPAPER", "paper-conference")
        }
        "dataset" | "data" => ("misc", "DATA", "dataset"),
        _ => ("article", "JOUR", "article-journal"),
    }
}

// ---------------------------------------------------------------------------
// BibTeX
// ---------------------------------------------------------------------------

/// Generate a BibTeX entry using the stored publication type.
pub fn to_bibtex(article: &Article) -> String {
    let key = cite_key(article);
    let mut lines = Vec::new();

    lines.push(format!("@{}{{{key},", entry_type(article).0));

    if !article.title.is_empty() {
        lines.push(format!("  title = {{{}}},", escape_bibtex(&article.title)));
    }

    if !article.authors.is_empty() {
        let names: Vec<String> = article
            .authors
            .iter()
            .map(|a| match (&a.fore_name, &a.initials) {
                (Some(f), _) => format!("{}, {}", escape_bibtex(&a.last_name), escape_bibtex(f)),
                (None, Some(i)) => format!("{}, {}", escape_bibtex(&a.last_name), escape_bibtex(i)),
                (None, None) => format!("{{{}}}", escape_bibtex(&a.last_name)),
            })
            .collect();
        lines.push(format!("  author = {{{}}},", names.join(" and ")));
    }

    if let Some(ref j) = article.journal {
        lines.push(format!("  journal = {{{}}},", escape_bibtex(j)));
    }
    if let Some(y) = article.year {
        lines.push(format!("  year = {{{y}}},"));
    }
    if let Some(ref v) = article.volume {
        lines.push(format!("  volume = {{{}}},", escape_bibtex(v)));
    }
    if let Some(ref i) = article.issue {
        lines.push(format!("  number = {{{}}},", escape_bibtex(i)));
    }
    if let Some(ref p) = article.pages {
        let pages = escape_bibtex(&p.replace('-', "--"));
        lines.push(format!("  pages = {{{pages}}},"));
    }
    if let Some(doi) = article.doi() {
        lines.push(format!("  doi = {{{}}},", escape_bibtex(doi)));
    }
    if let Some(pmid) = article.pmid() {
        lines.push(format!("  pmid = {{{}}},", escape_bibtex(pmid)));
    }

    if let Some(isbn) = article_isbn(article) {
        lines.push(format!("  isbn = {{{}}},", escape_bibtex(isbn)));
    }
    if let Some(url) = article_url(article) {
        lines.push(format!("  url = {{{}}},", escape_bibtex(url)));
    }
    for (key, field) in [
        ("publisher", "publisher"),
        ("place", "address"),
        ("edition", "edition"),
        ("book_title", "booktitle"),
        ("university", "school"),
        ("institution", "institution"),
        ("report_type", "type"),
        ("patent_number", "patentnumber"),
        ("access_date", "urldate"),
    ] {
        if let Some(value) = article.extra_fields.get(key) {
            lines.push(format!("  {field} = {{{}}},", escape_bibtex(value)));
        }
    }
    // Remove trailing comma from last field.
    if let Some(last) = lines.last_mut() {
        if last.ends_with(',') {
            last.pop();
        }
    }

    lines.push("}".to_string());
    lines.join("\n")
}

/// Build a cite key from first author's last name + year + first title word.
///
/// Delegates to `bib_types::evidence::cite_key` (the canonical home, so the
/// evidence payload layer can reuse it as its fuzzy dedup fallback without a
/// reverse dependency).
pub fn cite_key(article: &Article) -> String {
    bib_types::evidence::cite_key(article)
}

fn escape_bibtex(s: &str) -> String {
    let mut escaped = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => escaped.push_str("\\textbackslash{}"),
            '{' => escaped.push_str("\\{"),
            '}' => escaped.push_str("\\}"),
            '#' => escaped.push_str("\\#"),
            '$' => escaped.push_str("\\$"),
            '%' => escaped.push_str("\\%"),
            '&' => escaped.push_str("\\&"),
            '_' => escaped.push_str("\\_"),
            '^' => escaped.push_str("\\textasciicircum{}"),
            '~' => escaped.push_str("\\textasciitilde{}"),
            _ => escaped.push(c),
        }
    }
    escaped
}

// ---------------------------------------------------------------------------
// RIS
// ---------------------------------------------------------------------------

/// Generate an RIS entry.
pub fn to_ris(article: &Article) -> String {
    let mut lines = vec![format!("TY  - {}", entry_type(article).1)];

    for author in &article.authors {
        let name = match (&author.fore_name, &author.initials) {
            (Some(f), _) => format!("{}, {}", author.last_name, f),
            (None, Some(i)) => format!("{}, {}", author.last_name, i),
            (None, None) => author.last_name.clone(),
        };
        lines.push(format!("AU  - {name}"));
    }

    if !article.title.is_empty() {
        lines.push(format!("TI  - {}", article.title));
    }

    if let Some(ref j) = article.journal {
        lines.push(format!("JO  - {j}"));
    }
    if let Some(y) = article.year {
        lines.push(format!("PY  - {y}"));
    }
    if let Some(ref v) = article.volume {
        lines.push(format!("VL  - {v}"));
    }
    if let Some(ref i) = article.issue {
        lines.push(format!("IS  - {i}"));
    }
    if let Some(ref p) = article.pages {
        lines.push(format!("SP  - {p}"));
    }
    if let Some(ref doi) = article.doi() {
        lines.push(format!("DO  - {doi}"));
    }
    if let Some(ref pmid) = article.pmid() {
        lines.push(format!("ID  - {pmid}"));
    }
    if let Some(ref abs) = article.abstract_text {
        lines.push(format!("AB  - {abs}"));
    }

    if let Some(url) = article_url(article) {
        lines.push(format!("UR  - {url}"));
    }
    for keyword in &article.keywords {
        lines.push(format!("KW  - {keyword}"));
    }
    for (key, tag) in [
        ("publisher", "PB"),
        ("place", "CY"),
        ("edition", "ET"),
        ("book_title", "T2"),
        ("university", "PB"),
        ("institution", "PB"),
        ("access_date", "Y2"),
    ] {
        if let Some(value) = article.extra_fields.get(key) {
            lines.push(format!("{tag}  - {value}"));
        }
    }
    lines.push("ER  -".into());
    lines.join("\n")
}

// ---------------------------------------------------------------------------
// Markdown
// ---------------------------------------------------------------------------

/// Generate a compact Markdown citation (author-year style).
pub fn to_markdown(article: &Article) -> String {
    let author_str = match article.authors.len() {
        0 => "Anonymous".into(),
        1 => article.authors[0].display_name(),
        2 => format!(
            "{} & {}",
            article.authors[0].display_name(),
            article.authors[1].display_name()
        ),
        _ => format!("{} et al.", article.authors[0].display_name()),
    };

    let year = article
        .year
        .map(|y| y.to_string())
        .unwrap_or_else(|| "n.d.".into());

    let mut out = format!("- {author_str} ({year}). {}.", article.title);

    if let Some(ref j) = article.journal {
        out.push_str(&format!(" *{j}*"));
    }
    if let Some(ref v) = article.volume {
        out.push_str(&format!(", {v}"));
    }
    if let Some(ref p) = article.pages {
        out.push_str(&format!(":{p}"));
    }
    if let Some(ref doi) = article.doi() {
        out.push_str(&format!(". doi:[{doi}](https://doi.org/{doi})"));
    }

    out
}

// ---------------------------------------------------------------------------
// CSL-JSON
// ---------------------------------------------------------------------------

/// Convert to a Citation Style Language JSON object.
pub fn to_csl_json(article: &Article) -> serde_json::Value {
    let authors: Vec<serde_json::Value> = article
        .authors
        .iter()
        .map(|a| {
            serde_json::json!({
                "family": a.last_name,
                "given": a.fore_name,
            })
        })
        .collect();

    serde_json::json!({
        "id": article.id,
        "type": entry_type(article).2,
        "title": article.title,
        "author": authors,
        "issued": article.year.map(|y| {
            serde_json::json!({ "date-parts": [article.month.filter(|m| (1..=12).contains(m)).map(|m| vec![y, u16::from(m)]).unwrap_or_else(|| vec![y])] })
        }),
        "container-title": article.extra_fields.get("book_title").or_else(|| article.extra_fields.get("website_title")).map(String::as_str).or(article.journal.as_deref()),
        "publisher": article.extra_fields.get("publisher").or_else(||article.extra_fields.get("university")).or_else(||article.extra_fields.get("institution")),
        "publisher-place": article.extra_fields.get("place"),
        "edition": article.extra_fields.get("edition"),
        "genre": article.extra_fields.get("report_type"),
        "number": article.extra_fields.get("patent_number"),
        "accessed": article.extra_fields.get("access_date").map(|value|serde_json::json!({"raw":value})),
        "volume": article.volume,
        "issue": article.issue,
        "page": article.pages,
        "DOI": article.doi(),
        "URL": article_url(article),
        "keyword": article.keywords.join(", "),
        "PMID": article.pmid(),
        "ISBN": article_isbn(article),
        "abstract": article.abstract_text,
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use bib_types::{Article, ArticleSource, Author};

    fn sample() -> Article {
        let mut a = Article::new("test-1", "A breakthrough in CRISPR off-target detection");
        a.authors.push(Author {
            last_name: "Smith".into(),
            fore_name: Some("John A".into()),
            initials: Some("JA".into()),
            affiliation: None,
            orcid: None,
            corresponding: false,
        });
        a.authors.push(Author {
            last_name: "Jones".into(),
            fore_name: None,
            initials: Some("B".into()),
            affiliation: None,
            orcid: None,
            corresponding: false,
        });
        a.year = Some(2024);
        a.journal = Some("Nature Genetics".into());
        a.volume = Some("56".into());
        a.issue = Some("3".into());
        a.pages = Some("100-110".into());
        a.abstract_text = Some("We describe a novel method.".into());
        a.identifiers
            .push(bib_types::Identifier::doi("10.1000/test"));
        a.identifiers.push(bib_types::Identifier::pmid("30124452"));
        a.source = ArticleSource::Pubmed;
        a
    }

    #[test]
    fn bibtex_contains_key_fields() {
        let bib = to_bibtex(&sample());
        assert!(bib.starts_with("@article{smith2024a,"));
        assert!(bib.contains("title = {A breakthrough in CRISPR off-target detection}"));
        assert!(bib.contains("author = {Smith, John A and Jones, B}"));
        assert!(bib.contains("journal = {Nature Genetics}"));
        assert!(bib.contains("year = {2024}"));
        assert!(bib.contains("volume = {56}"));
        assert!(bib.contains("pages = {100--110}"));
        assert!(bib.contains("doi = {10.1000/test}"));
        assert!(bib.contains("pmid = {30124452}"));
    }

    #[test]
    fn bibtex_escapes_special_characters() {
        let mut article = sample();
        article.title = "R&D at 100%: A_B {C} #1, $2, C^D ~E, F\\G".into();
        article.journal = Some("Journal of R&D Systems".into());
        article.authors[0].last_name = "O'Neill & Co".into();
        article.volume = Some("10_20".into());
        article.issue = Some("Issue #3".into());
        article.pages = Some("1&2-3_4".into());
        article.identifiers.clear();
        article
            .identifiers
            .push(bib_types::Identifier::doi("10.1000/r&_d_2024#1"));

        let bib = to_bibtex(&article);

        assert!(bib.starts_with("@article{oneillco2024rd,"));

        assert!(bib.contains("title = {R\\&D at 100\\%: A\\_B \\{C\\} \\#1, \\$2, C\\textasciicircum{}D \\textasciitilde{}E, F\\textbackslash{}G},"));
        assert!(bib.contains("journal = {Journal of R\\&D Systems}"));
        assert!(bib.contains("author = {O'Neill \\& Co, John A and Jones, B}"));
        assert!(bib.contains("volume = {10\\_20}"));
        assert!(bib.contains("number = {Issue \\#3}"));
        assert!(bib.contains("pages = {1\\&2--3\\_4}"));
        assert!(bib.contains("doi = {10.1000/r\\&\\_d\\_2024\\#1}"));
    }

    #[test]
    fn ris_contains_fields() {
        let ris = to_ris(&sample());
        assert!(ris.starts_with("TY  - JOUR"));
        assert!(ris.contains("AU  - Smith, John A"));
        assert!(ris.contains("TI  - A breakthrough"));
        assert!(ris.contains("DO  - 10.1000/test"));
        assert!(ris.contains("ER  -"));
    }

    #[test]
    fn markdown_is_compact() {
        let md = to_markdown(&sample());
        assert!(md.starts_with("- Smith John A & Jones B (2024)."));
        assert!(md.contains("*Nature Genetics*"));
        assert!(md.contains("doi:[10.1000/test]"));
    }

    #[test]
    fn cite_key_generation() {
        assert_eq!(cite_key(&sample()), "smith2024a");

        let mut no_author = Article::new("x", "Hello World");
        no_author.year = Some(2020);
        assert_eq!(cite_key(&no_author), "anon2020hello");

        // Trailing punctuation in the title's first word (e.g. "Tutorial:")
        // must not leak into the cite key.
        let mut titled = Article::new("x", "Tutorial: A Guide to GWAS");
        titled.authors.push(bib_types::Author {
            last_name: "Choi".into(),
            fore_name: None,
            initials: Some("M".into()),
            affiliation: None,
            orcid: None,
            corresponding: false,
        });
        titled.year = Some(2020);
        assert_eq!(cite_key(&titled), "choi2020tutorial");
    }

    #[test]
    fn csl_collection_is_valid_array_with_precise_dates_and_types() {
        let mut article = Article::new("book", "A Book");
        article.pub_types = vec!["Book".into()];
        article.year = Some(2024);
        article.identifiers.push(bib_types::Identifier::new(
            bib_types::IdKind::Other,
            "https://example.org/book",
        ));
        let output = render_all(
            &[article.clone(), Article::new("two", "Second")],
            ExportFormat::CslJson,
        );
        let value: serde_json::Value = serde_json::from_str(&output).unwrap();
        assert_eq!(value.as_array().unwrap().len(), 2);
        assert_eq!(
            value[0]["issued"]["date-parts"][0],
            serde_json::json!([2024])
        );
        assert_eq!(value[0]["type"], "book");
        assert_eq!(value[0]["URL"], "https://example.org/book");
        assert!(to_bibtex(&article).starts_with("@book{"));
        assert!(to_ris(&article).starts_with("TY  - BOOK"));
    }

    #[test]
    fn render_all_joins_entries() {
        let a1 = sample();
        let a2 = Article::new("t2", "Second paper");
        let out = render_all(&[a1, a2], ExportFormat::Bibtex);
        assert_eq!(out.matches("@article").count(), 2);
    }

    #[test]
    fn csl_json_has_required_fields() {
        let json = to_csl_json(&sample());
        assert_eq!(json["type"], "article-journal");
        assert_eq!(json["author"][0]["family"], "Smith");
        assert_eq!(json["DOI"], "10.1000/test");
    }
}
