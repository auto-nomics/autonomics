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
    articles
        .iter()
        .map(|a| render(a, format))
        .collect::<Vec<_>>()
        .join("\n\n")
}

// ---------------------------------------------------------------------------
// BibTeX
// ---------------------------------------------------------------------------

/// Generate a BibTeX `@article` entry.
pub fn to_bibtex(article: &Article) -> String {
    let key = cite_key(article);
    let mut lines = Vec::new();

    lines.push(format!("@article{{{key},"));

    if !article.title.is_empty() {
        lines.push(format!("  title = {{{}}},", escape_bibtex(&article.title)));
    }

    if !article.authors.is_empty() {
        let names: Vec<String> = article
            .authors
            .iter()
            .map(|a| match (&a.fore_name, &a.initials) {
                (Some(f), _) => format!("{} {}", escape_bibtex(f), escape_bibtex(&a.last_name)),
                (None, Some(i)) => format!("{} {}", escape_bibtex(&a.last_name), escape_bibtex(i)),
                (None, None) => escape_bibtex(&a.last_name),
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
pub fn cite_key(article: &Article) -> String {
    let author_part = article
        .authors
        .first()
        .map(|a| {
            a.last_name
                .to_lowercase()
                .chars()
                .filter(|c| c.is_ascii_alphanumeric())
                .collect::<String>()
        })
        .filter(|part| !part.is_empty())
        .unwrap_or_else(|| "anon".into());
    let year_part = article
        .year
        .map(|y| y.to_string())
        .unwrap_or_else(|| "nd".into());
    let title_part = article
        .title
        .split_whitespace()
        .next()
        .map(|w| {
            // Keep only alphanumeric characters so trailing punctuation
            // (e.g. "Tutorial:" → "tutorial") doesn't leak into the key.
            w.to_lowercase()
                .chars()
                .filter(|c| c.is_ascii_alphanumeric())
                .collect::<String>()
        })
        .filter(|w| !w.is_empty())
        .unwrap_or_else(|| "untitled".into());
    format!("{author_part}{year_part}{title_part}")
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
    let mut lines = vec!["TY  - JOUR".to_string()];

    for author in &article.authors {
        let name = match (&author.fore_name, &author.initials) {
            (Some(f), _) => format!("{} {}", author.last_name, f),
            (None, Some(i)) => format!("{} {}", author.last_name, i),
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
        "type": "article-journal",
        "title": article.title,
        "author": authors,
        "issued": article.year.map(|y| {
            serde_json::json!({ "date-parts": [[y, article.month.unwrap_or(0)]] })
        }),
        "container-title": article.journal,
        "volume": article.volume,
        "issue": article.issue,
        "page": article.pages,
        "DOI": article.doi(),
        "PMID": article.pmid(),
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
        assert!(bib.contains("author = {John A Smith and Jones B}"));
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
        assert!(bib.contains("author = {John A O'Neill \\& Co and Jones B}"));
        assert!(bib.contains("volume = {10\\_20}"));
        assert!(bib.contains("number = {Issue \\#3}"));
        assert!(bib.contains("pages = {1\\&2--3\\_4}"));
        assert!(bib.contains("doi = {10.1000/r\\&\\_d\\_2024\\#1}"));
    }

    #[test]
    fn ris_contains_fields() {
        let ris = to_ris(&sample());
        assert!(ris.starts_with("TY  - JOUR"));
        assert!(ris.contains("AU  - Smith John A"));
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
