//! Render bioRxiv/medRxiv API responses into clean, LLM-friendly Markdown.

use crate::types::{BiorxivEntry, DetailsResponse, Message, PubMapping, PubResponse};

// ---------------------------------------------------------------------------
// Details responses
// ---------------------------------------------------------------------------

/// Format a [`DetailsResponse`] into readable Markdown with article previews.
///
/// If the collection contains multiple versions of the same DOI, each version
/// is listed separately.
pub fn format_details(resp: &DetailsResponse) -> String {
    let mut out = String::with_capacity(4096);

    let total = resp
        .messages
        .first()
        .and_then(|m| m.total)
        .unwrap_or(resp.collection.len() as u64);

    out.push_str(&format!("**{total}** papers"));
    if resp.collection.len() as u64 != total {
        out.push_str(&format!(" (showing {})", resp.collection.len()));
    }
    out.push_str("\n\n");

    for entry in &resp.collection {
        format_entry_preview(entry, &mut out);
        out.push('\n');
    }

    out.trim_end().to_string()
}

/// Format entries with full detail (abstract, all metadata).
pub fn format_entries_full(entries: &[BiorxivEntry]) -> String {
    let mut out = String::with_capacity(8192);

    out.push_str(&format!("**{} articles**\n\n", entries.len()));

    for entry in entries {
        format_entry_full(entry, &mut out);
        out.push('\n');
    }

    out.trim_end().to_string()
}

// ---------------------------------------------------------------------------
// Pub responses
// ---------------------------------------------------------------------------

/// Format a [`PubResponse`] into a compact table of preprint → published mappings.
pub fn format_pub(resp: &PubResponse) -> String {
    let mut out = String::with_capacity(2048);

    let total = resp
        .messages
        .first()
        .and_then(|m| m.total)
        .unwrap_or(resp.collection.len() as u64);

    out.push_str(&format!("**{total}** published-article mappings"));
    if resp.collection.len() as u64 != total {
        out.push_str(&format!(" (showing {})", resp.collection.len()));
    }
    out.push_str("\n\n");

    for m in &resp.collection {
        format_pub_mapping(m, &mut out);
        out.push('\n');
    }

    out.trim_end().to_string()
}

// ---------------------------------------------------------------------------
// Per-entry formatters
// ---------------------------------------------------------------------------

fn format_entry_preview(entry: &BiorxivEntry, out: &mut String) {
    let server_label = server_label(&entry.server);
    out.push_str(&format!(
        "### [{server_label}:{doi}] {title}\n",
        doi = entry.doi,
        title = entry.title,
    ));

    // Authors (first 5, then "et al.")
    let author_names: Vec<&str> = entry
        .authors
        .split(';')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();
    if !author_names.is_empty() {
        let display = if author_names.len() > 5 {
            format!("{}, et al.", author_names[..5].join(", "))
        } else {
            author_names.join(", ")
        };
        out.push_str(&format!("**Authors:** {display}\n"));
    }

    // Category
    if !entry.category.is_empty() && entry.category != "NA" {
        out.push_str(&format!("**Category:** {}\n", entry.category));
    }

    // Date + version
    if !entry.date.is_empty() {
        out.push_str(&format!("**Date:** {} (v{})\n", entry.date, entry.version));
    }

    // Published DOI
    if !entry.published.is_empty() && entry.published != "NA" {
        out.push_str(&format!("**Published DOI:** {}\n", entry.published));
    }

    // Abstract (truncated for preview)
    let abs = entry.abstract_text.trim();
    if !abs.is_empty() && abs != "NA" {
        let preview = truncate_str(abs, 300);
        out.push_str(&format!("\n> {preview}\n"));
    }

    out.push('\n');
}

fn format_entry_full(entry: &BiorxivEntry, out: &mut String) {
    let server_label = server_label(&entry.server);
    out.push_str(&format!(
        "### [{server_label}:{doi}] {title}\n",
        doi = entry.doi,
        title = entry.title,
    ));

    // Authors
    if !entry.authors.is_empty() {
        out.push_str(&format!("**Authors:** {}\n", entry.authors));
    }

    // Corresponding author + institution
    if let Some(ref corr) = entry.author_corresponding {
        out.push_str(&format!("**Corresponding:** {corr}"));
        if let Some(ref inst) = entry.author_corresponding_institution {
            if !inst.is_empty() && inst != "NA" {
                out.push_str(&format!(" ({inst})"));
            }
        }
        out.push('\n');
    }

    // Category + type
    if !entry.category.is_empty() && entry.category != "NA" {
        out.push_str(&format!("**Category:** {}\n", entry.category));
    }
    if !entry.article_type.is_empty() && entry.article_type != "NA" {
        out.push_str(&format!("**Type:** {}\n", entry.article_type));
    }

    // Date + version
    if !entry.date.is_empty() {
        out.push_str(&format!(
            "**Date:** {} (version {})\n",
            entry.date, entry.version
        ));
    }

    // License
    if !entry.license.is_empty() && entry.license != "NA" {
        out.push_str(&format!("**License:** {}\n", entry.license));
    }

    // Published DOI
    if !entry.published.is_empty() && entry.published != "NA" {
        out.push_str(&format!("**Published DOI:** {}\n", entry.published));
    }

    // Funder
    if !entry.funder.is_empty() && entry.funder != "NA" {
        out.push_str(&format!("**Funding:** {}\n", entry.funder));
    }

    // JATS XML link
    if !entry.jatsxml.is_empty() {
        out.push_str(&format!("**JATS XML:** {}\n", entry.jatsxml));
    }

    // Full abstract
    let abs = entry.abstract_text.trim();
    if !abs.is_empty() && abs != "NA" {
        out.push('\n');
        out.push_str(abs);
        out.push_str("\n\n");
    }

    out.push('\n');
}

fn format_pub_mapping(m: &PubMapping, out: &mut String) {
    out.push_str(&format!("### {} → {}\n", m.biorxiv_doi, m.published_doi));
    out.push_str(&format!("**Title:** {}\n", m.preprint_title));
    if !m.preprint_category.is_empty() {
        out.push_str(&format!("**Category:** {}\n", m.preprint_category));
    }
    if !m.preprint_date.is_empty() {
        out.push_str(&format!("**Preprint date:** {}\n", m.preprint_date));
    }
    if !m.published_date.is_empty() {
        out.push_str(&format!("**Published:** {}\n", m.published_date));
    }
    out.push('\n');
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Normalise the server field to a display label.
fn server_label(server: &str) -> &str {
    if server.eq_ignore_ascii_case("medrxiv") {
        "medRxiv"
    } else if server.eq_ignore_ascii_case("biorxiv") {
        "bioRxiv"
    } else if server.is_empty() {
        "preprint"
    } else {
        "preprint"
    }
}

/// Truncate a string to `max_len` characters at a word boundary, appending
/// "…" if truncated.
fn truncate_str(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        return s.to_owned();
    }
    let mut end = max_len;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    let trimmed = &s[..end];
    let cut = if let Some(space) = trimmed.rfind(' ') {
        &s[..space]
    } else {
        trimmed
    };
    format!("{cut}…")
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_entry() -> BiorxivEntry {
        BiorxivEntry {
            doi: "10.1101/2024.01.01.573421".into(),
            title: "A Comprehensive Genomic Study".into(),
            authors: "Smith, J.; Doe, K.; Roe, R.".into(),
            author_corresponding: Some("Jane Smith".into()),
            author_corresponding_institution: Some("MIT".into()),
            date: "2024-01-01".into(),
            version: "2".into(),
            article_type: "new_result".into(),
            license: "cc_by".into(),
            category: "genetics".into(),
            jatsxml: "https://www.medrxiv.org/content/source.xml".into(),
            abstract_text: "This study presents a comprehensive analysis of genomic variation across populations. We identify novel loci associated with disease.".into(),
            published: "NA".into(),
            server: "medrxiv".into(),
            funder: "NIH".into(),
        }
    }

    #[test]
    fn format_details_basic() {
        let resp = DetailsResponse {
            messages: vec![Message {
                status: "ok".into(),
                total: Some(42),
                count: Some(1),
                ..Default::default()
            }],
            collection: vec![sample_entry()],
        };
        let rendered = format_details(&resp);
        assert!(rendered.contains("**42** papers"));
        assert!(rendered.contains("[medRxiv:10.1101/2024.01.01.573421]"));
        assert!(rendered.contains("Comprehensive Genomic Study"));
        assert!(rendered.contains("Smith, J."));
        assert!(rendered.contains("Category:** genetics"));
        assert!(rendered.contains("(v2)"));
    }

    #[test]
    fn format_entries_full_basic() {
        let rendered = format_entries_full(&[sample_entry()]);
        assert!(rendered.contains("**1 articles**"));
        assert!(rendered.contains("Corresponding:** Jane Smith (MIT)"));
        assert!(rendered.contains("License:** cc_by"));
        assert!(rendered.contains("Funding:** NIH"));
        assert!(rendered.contains("genomic variation"));
    }

    #[test]
    fn format_pub_basic() {
        let resp = PubResponse {
            messages: vec![Message {
                status: "ok".into(),
                total: Some(1),
                ..Default::default()
            }],
            collection: vec![PubMapping {
                biorxiv_doi: "10.1101/2022.09.11.507474".into(),
                published_doi: "10.1038/s41564-023-01548-y".into(),
                preprint_title: "A new route for integron cassette dissemination".into(),
                preprint_category: "genetics".into(),
                preprint_date: "2022-09-13".into(),
                published_date: "2024-01-03".into(),
            }],
        };
        let rendered = format_pub(&resp);
        assert!(rendered.contains("**1** published-article mappings"));
        assert!(rendered.contains("10.1038/s41564-023-01548-y"));
        assert!(rendered.contains("integron cassette"));
    }

    #[test]
    fn truncate_long() {
        let s = "This is a very long sentence that should be truncated at a word boundary.";
        let t = truncate_str(s, 30);
        assert!(t.ends_with('…'));
    }

    #[test]
    fn truncate_short() {
        assert_eq!(truncate_str("short", 100), "short");
    }

    #[test]
    fn server_label_works() {
        assert_eq!(server_label("medrxiv"), "medRxiv");
        assert_eq!(server_label("medRxiv"), "medRxiv");
        assert_eq!(server_label("biorxiv"), "bioRxiv");
        assert_eq!(server_label("bioRxiv"), "bioRxiv");
    }
}
