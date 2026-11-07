//! Deck citation support: `{{cite:key}}` scanning and References slide
//! generation, bridged to the shared `bib-base` library (same cite-key
//! convention as writing-base: `bib_base::cite_key` with `b`/`c`…
//! disambiguation suffixes).

use std::collections::BTreeMap;
use std::sync::Arc;

use bib_base::BibBase;
use bib_types::Article;

/// `{{cite:key}}` — deck slide citation marker.
pub fn extract_cite_keys(markdown: &str) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let mut keys = Vec::new();
    let mut rest = markdown;
    while let Some(start) = rest.find("{{cite:") {
        let after = &rest[start + 7..];
        if let Some(end) = after.find("}}") {
            let key = after[..end].trim().to_string();
            if !key.is_empty() && seen.insert(key.clone()) {
                keys.push(key);
            }
            rest = &after[end + 2..];
        } else {
            break;
        }
    }
    keys
}

/// Cite-key → article resolver over the shared library.
pub struct DeckCitationResolver {
    bib: Arc<BibBase>,
}

impl DeckCitationResolver {
    pub fn new(bib: Arc<BibBase>) -> Self {
        Self { bib }
    }

    /// Build the cite-key index (same disambiguation rules as
    /// writing-base's resolver).
    async fn build_index(&self) -> BTreeMap<String, String> {
        let articles = self.bib.list_all_articles().await.unwrap_or_default();
        let mut groups: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for article in &articles {
            let base = bib_base::cite_key(article);
            groups.entry(base).or_default().push(article.id.clone());
        }
        let mut forward = BTreeMap::new();
        for (base, ids) in groups {
            for (i, id) in ids.iter().enumerate() {
                let key = if i == 0 {
                    base.clone()
                } else {
                    format!("{}{}", base, (b'b' + (i as u8 - 1)) as char)
                };
                forward.insert(key, id.clone());
            }
        }
        forward
    }

    /// Resolve each key to its article (`None` = unresolved).
    pub async fn resolve_keys(&self, keys: &[String]) -> Vec<(String, Option<Article>)> {
        let index = self.build_index().await;
        let mut out = Vec::with_capacity(keys.len());
        for key in keys {
            let article = match index.get(key) {
                Some(id) => self.bib.get_article(id).await.ok().flatten(),
                None => None,
            };
            out.push((key.clone(), article));
        }
        out
    }
}

/// One numbered reference line (slide-compact format).
pub fn format_slide_reference(article: &Article) -> String {
    let mut authors: Vec<String> = article
        .authors
        .iter()
        .map(|a| match (&a.fore_name, &a.last_name) {
            (Some(f), l) if !f.is_empty() => format!("{} {}", initials(f), l),
            (_, l) => l.clone(),
        })
        .collect();
    let et_al = if authors.len() > 3 {
        authors.truncate(3);
        " et al.".to_string()
    } else {
        String::new()
    };
    let author_part = if authors.is_empty() {
        "[anonymous]".to_string()
    } else {
        format!("{}{et_al}.", authors.join(", "))
    };
    let journal = article
        .journal
        .as_deref()
        .map(|j| format!(" *{j}*."))
        .unwrap_or_default();
    let year = article
        .year
        .map(|y| format!(" {y}."))
        .unwrap_or_else(|| ".".to_string());
    let doi = article
        .identifiers
        .iter()
        .find(|i| matches!(i.kind, bib_types::IdKind::Doi))
        .map(|i| format!(" doi:{}", i.value))
        .unwrap_or_default();
    format!("{} {}{}{}{}", author_part, article.title, journal, year, doi)
}

fn initials(fore_name: &str) -> String {
    fore_name
        .split_whitespace()
        .filter_map(|p| p.chars().next())
        .map(|c| format!("{c}."))
        .collect()
}

/// The References slide's canonical markdown (layout: references).
pub fn references_slide_markdown(entries: &[String]) -> String {
    let mut md = String::from("## 参考文献\n");
    for (i, entry) in entries.iter().enumerate() {
        md.push_str(&format!("\n{}. {}", i + 1, entry));
    }
    md.push('\n');
    md
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_keys_in_order_and_dedups() {
        let md = "a {{cite:smith2024}} b {{cite:wayne1939}} again {{cite:smith2024}} {{cite:x\ny}}";
        assert_eq!(
            extract_cite_keys(md),
            ["smith2024", "wayne1939", "x\ny"]
        );
        assert!(extract_cite_keys("no markers").is_empty());
        assert!(extract_cite_keys("dangling {{cite:open").is_empty());
    }

    #[test]
    fn formats_reference_compactly() {
        let article = Article {
            id: "a1".into(),
            title: "A very long study".into(),
            authors: vec![
                bib_types::Author {
                    last_name: "Smith".into(),
                    fore_name: Some("Jane".into()),
                    initials: None,
                    affiliation: None,
                    orcid: None,
                    corresponding: false,
                },
                bib_types::Author {
                    last_name: "Wu".into(),
                    fore_name: Some("Ming".into()),
                    initials: None,
                    affiliation: None,
                    orcid: None,
                    corresponding: false,
                },
                bib_types::Author {
                    last_name: "Lee".into(),
                    fore_name: None,
                    initials: None,
                    affiliation: None,
                    orcid: None,
                    corresponding: false,
                },
                bib_types::Author {
                    last_name: "Garcia".into(),
                    fore_name: Some("P".into()),
                    initials: None,
                    affiliation: None,
                    orcid: None,
                    corresponding: false,
                },
            ],
            identifiers: vec![bib_types::Identifier::new(bib_types::IdKind::Doi, "10.1234/x")],
            year: Some(2024),
            journal: Some("Nature".into()),
            abstract_text: None,
            month: None,
            volume: None,
            issue: None,
            pages: None,
            issn: None,
            essn: None,
            language: None,
            pub_types: Vec::new(),
            keywords: Vec::new(),
            source: Default::default(),
            created_at: None,
            updated_at: None,
        };
        let line = format_slide_reference(&article);
        assert!(line.starts_with("J. Smith, M. Wu, Lee et al."));
        assert!(line.contains("*Nature*. 2024."));
        assert!(line.contains("doi:10.1234/x"));
    }

    #[test]
    fn references_slide_shape() {
        let md = references_slide_markdown(&["One".into(), "Two".into()]);
        assert!(md.starts_with("## 参考文献\n"));
        assert!(md.contains("\n1. One"));
        assert!(md.contains("\n2. Two"));
    }
}
