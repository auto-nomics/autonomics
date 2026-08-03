//! Render arXiv API responses into clean, LLM-friendly Markdown.

use crate::types::{ArxivEntry, SearchResponse};

// ---------------------------------------------------------------------------
// Search results
// ---------------------------------------------------------------------------

/// Format an arXiv [`SearchResponse`] into readable Markdown with article
/// previews.
pub fn format_search(resp: &SearchResponse) -> String {
    let mut out = String::with_capacity(4096);

    out.push_str(&format!("**{}** results found", resp.total_results));
    if resp.entries.len() < resp.total_results as usize {
        out.push_str(&format!(" (showing {})", resp.entries.len()));
    }
    out.push_str("\n\n");

    for entry in &resp.entries {
        format_entry_preview(entry, &mut out);
        out.push('\n');
    }

    out.trim_end().to_string()
}

// ---------------------------------------------------------------------------
// Fetch results (full detail)
// ---------------------------------------------------------------------------

/// Format arXiv entries with full detail (abstract, all metadata).
pub fn format_entries_full(entries: &[ArxivEntry]) -> String {
    let mut out = String::with_capacity(8192);

    out.push_str(&format!("**{} articles**\n\n", entries.len()));

    for entry in entries {
        format_entry_full(entry, &mut out);
        out.push('\n');
    }

    out.trim_end().to_string()
}

// ---------------------------------------------------------------------------
// Per-entry formatters
// ---------------------------------------------------------------------------

fn format_entry_preview(entry: &ArxivEntry, out: &mut String) {
    out.push_str(&format!("### [arXiv:{}] {}\n", entry.arxiv_id, entry.title));

    // Authors (first 5, then "et al.")
    let names: Vec<&str> = entry.authors.iter().map(|a| a.name.as_str()).collect();
    if !names.is_empty() {
        let display = if names.len() > 5 {
            format!("{}, et al.", names[..5].join(", "))
        } else {
            names.join(", ")
        };
        out.push_str(&format!("**Authors:** {display}\n"));
    }

    // Primary category
    if let Some(ref cat) = entry.primary_category {
        out.push_str(&format!("**Category:** {cat}\n"));
    }

    // Published date
    if let Some(ref published) = entry.published {
        let date = published.get(0..10).unwrap_or(published);
        out.push_str(&format!("**Published:** {date}\n"));
    }

    // DOI
    if let Some(ref doi) = entry.doi {
        out.push_str(&format!("**DOI:** {doi}\n"));
    }

    // Abstract (truncated for preview)
    if let Some(ref summary) = entry.summary {
        let preview = truncate_str(summary, 300);
        out.push_str(&format!("\n> {preview}\n"));
    }

    out.push('\n');
}

fn format_entry_full(entry: &ArxivEntry, out: &mut String) {
    out.push_str(&format!("### [arXiv:{}] {}\n", entry.arxiv_id, entry.title));

    // Authors with affiliations
    if !entry.authors.is_empty() {
        out.push_str("**Authors:** ");
        let parts: Vec<String> = entry
            .authors
            .iter()
            .map(|a| {
                if let Some(ref aff) = a.affiliation {
                    if !aff.is_empty() {
                        return format!("{} ({})", a.name, aff);
                    }
                }
                a.name.clone()
            })
            .collect();
        out.push_str(&parts.join(", "));
        out.push('\n');
    }

    // Primary category + all categories
    if let Some(ref cat) = entry.primary_category {
        out.push_str(&format!("**Primary category:** {cat}\n"));
    }
    if !entry.categories.is_empty() {
        out.push_str(&format!(
            "**Categories:** {}\n",
            entry.categories.join(", ")
        ));
    }

    // Dates
    if let Some(ref published) = entry.published {
        let date = published.get(0..10).unwrap_or(published);
        out.push_str(&format!("**Published:** {date}\n"));
    }
    if let Some(ref updated) = entry.updated {
        let date = updated.get(0..10).unwrap_or(updated);
        if Some(updated.as_str()) != entry.published.as_deref() {
            out.push_str(&format!("**Updated:** {date}\n"));
        }
    }

    // DOI
    if let Some(ref doi) = entry.doi {
        out.push_str(&format!("**DOI:** {doi}\n"));
    }

    // Journal reference
    if let Some(ref jr) = entry.journal_ref {
        if !jr.is_empty() {
            out.push_str(&format!("**Journal ref:** {jr}\n"));
        }
    }

    // Comment
    if let Some(ref comment) = entry.comment {
        if !comment.is_empty() {
            out.push_str(&format!("**Comments:** {comment}\n"));
        }
    }

    // Links
    if let Some(ref pdf) = entry.pdf_url {
        out.push_str(&format!("**PDF:** {pdf}\n"));
    }
    if let Some(ref abs) = entry.abs_url {
        out.push_str(&format!("**Abstract:** {abs}\n"));
    }

    // Full abstract
    if let Some(ref summary) = entry.summary {
        if !summary.is_empty() {
            out.push('\n');
            out.push_str(summary);
            out.push_str("\n\n");
        }
    }

    out.push('\n');
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

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
    use crate::types::{ArxivAuthor, SearchResponse};

    fn sample_entry() -> ArxivEntry {
        ArxivEntry {
            arxiv_id: "2401.12345v2".into(),
            title: "Attention Is All You Need".into(),
            summary: Some(
                "This paper introduces the Transformer, a new architecture for \
                 sequence transduction based solely on attention mechanisms."
                    .into(),
            ),
            authors: vec![
                ArxivAuthor {
                    name: "Ashish Vaswani".into(),
                    affiliation: Some("Google Brain".into()),
                },
                ArxivAuthor {
                    name: "Noam Shazeer".into(),
                    affiliation: None,
                },
            ],
            published: Some("2024-01-15T00:00:00Z".into()),
            updated: Some("2024-01-20T12:00:00Z".into()),
            primary_category: Some("cs.LG".into()),
            categories: vec!["cs.LG".into(), "cs.CL".into()],
            doi: Some("10.1000/test".into()),
            journal_ref: Some("Nature 2024".into()),
            comment: Some("15 pages, 3 figures".into()),
            pdf_url: Some("http://arxiv.org/pdf/2401.12345v2".into()),
            abs_url: Some("http://arxiv.org/abs/2401.12345v2".into()),
        }
    }

    #[test]
    fn format_search_basic() {
        let resp = SearchResponse {
            total_results: 42,
            start_index: 0,
            items_per_page: 2,
            entries: vec![sample_entry()],
        };
        let rendered = format_search(&resp);
        assert!(rendered.contains("**42** results"));
        assert!(rendered.contains("[arXiv:2401.12345v2]"));
        assert!(rendered.contains("Attention Is All You Need"));
        assert!(rendered.contains("Vaswani"));
        assert!(rendered.contains("Category:** cs.LG"));
        assert!(rendered.contains("Published:** 2024-01-15"));
        assert!(rendered.contains("DOI:** 10.1000/test"));
    }

    #[test]
    fn format_entries_full_basic() {
        let rendered = format_entries_full(&[sample_entry()]);
        assert!(rendered.contains("**1 articles**"));
        assert!(rendered.contains("Vaswani (Google Brain)"));
        assert!(rendered.contains("Shazeer"));
        assert!(rendered.contains("Categories:** cs.LG, cs.CL"));
        assert!(rendered.contains("Journal ref:** Nature 2024"));
        assert!(rendered.contains("Comments:** 15 pages"));
        assert!(rendered.contains("Transformer"));
    }

    #[test]
    fn truncate_long_string() {
        let s = "This is a very long sentence that should be truncated at a word boundary.";
        let t = truncate_str(s, 30);
        assert!(t.ends_with('…'));
        assert!(t.len() <= 32); // 30 + ellipsis
    }

    #[test]
    fn truncate_short_string() {
        assert_eq!(truncate_str("short", 100), "short");
    }
}
