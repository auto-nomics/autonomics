//! Render Embase API responses into clean, LLM-friendly Markdown.

use crate::types::{SearchEntry, SearchResponse};

// ---------------------------------------------------------------------------
// Search results
// ---------------------------------------------------------------------------

/// Format an Embase search response into readable Markdown.
pub fn format_search(resp: &SearchResponse) -> String {
    // Check for API-level error.
    if let Some(ref err) = resp.error {
        if let Some(ref detail) = err.detail {
            return format!(
                "**Embase API error** (status: {}): {}\n",
                err.status.as_deref().unwrap_or("?"),
                detail
            );
        }
    }

    let total = resp.total_results.parse::<u64>().unwrap_or(0);
    let n_entries = resp.entry.len() as u64;

    let mut out = String::with_capacity(4096);
    out.push_str(&format!("**{}** results found", total));
    if n_entries > 0 {
        out.push_str(&format!(" (showing {})\n\n", n_entries));
    } else {
        out.push_str("\n\nNo entries returned.\n");
        return out;
    }

    for (i, entry) in resp.entry.iter().enumerate() {
        out.push_str(&format!(
            "### {}. {}\n",
            i + 1,
            entry_title_or_fallback(entry)
        ));

        // Authors
        if let Some(ref creator) = entry.creator {
            if !creator.is_empty() {
                out.push_str(&format!("**Author:** {}\n", creator));
            }
        }

        // Journal
        if let Some(ref name) = entry.publication_name {
            if !name.is_empty() {
                let mut parts: Vec<String> = Vec::new();
                if let Some(ref v) = entry.volume {
                    if !v.is_empty() {
                        parts.push(v.clone());
                    }
                }
                if let Some(ref iss) = entry.issue_identifier {
                    if !iss.is_empty() {
                        parts.push(format!("({})", iss));
                    }
                }
                if parts.is_empty() {
                    out.push_str(&format!("**Journal:** {}\n", name));
                } else {
                    out.push_str(&format!("**Journal:** {}, {}\n", name, parts.join(" ")));
                }
            }
        }

        // Date
        if let Some(ref date) = entry.cover_date {
            if !date.is_empty() {
                out.push_str(&format!("**Date:** {}\n", date));
            }
        }

        // DOI
        if let Some(ref doi) = entry.doi {
            if !doi.is_empty() {
                out.push_str(&format!("**DOI:** {}\n", doi));
            }
        }

        // Pages
        if let Some(ref pages) = entry.page_range {
            if !pages.is_empty() {
                out.push_str(&format!("**Pages:** {}\n", pages));
            }
        }

        // Abstract (truncated for search results)
        if let Some(ref desc) = entry.description {
            if !desc.is_empty() {
                let cleaned = strip_html_tags(desc);
                let truncated = truncate_at(&cleaned, 500);
                out.push('\n');
                out.push_str(truncated);
                if cleaned.len() > 500 {
                    out.push_str("...");
                }
                out.push_str("\n\n");
            }
        }

        out.push('\n');
    }

    out.trim_end().to_string()
}

// ---------------------------------------------------------------------------
// Retrieval result
// ---------------------------------------------------------------------------

/// Format a single Embase retrieval entry into readable Markdown.
pub fn format_retrieval(entry: &SearchEntry) -> String {
    let mut out = String::with_capacity(4096);
    out.push_str(&format!("## {}\n\n", entry_title_or_fallback(entry)));

    if let Some(ref creator) = entry.creator {
        if !creator.is_empty() {
            out.push_str(&format!("**Author:** {}\n", creator));
        }
    }

    if let Some(ref name) = entry.publication_name {
        if !name.is_empty() {
            let mut parts: Vec<String> = Vec::new();
            if let Some(ref v) = entry.volume {
                if !v.is_empty() {
                    parts.push(v.clone());
                }
            }
            if let Some(ref iss) = entry.issue_identifier {
                if !iss.is_empty() {
                    parts.push(format!("({})", iss));
                }
            }
            if parts.is_empty() {
                out.push_str(&format!("**Journal:** {}\n", name));
            } else {
                out.push_str(&format!("**Journal:** {}, {}\n", name, parts.join(" ")));
            }
        }
    }

    if let Some(ref date) = entry.cover_date {
        if !date.is_empty() {
            out.push_str(&format!("**Date:** {}\n", date));
        }
    }
    if let Some(ref doi) = entry.doi {
        if !doi.is_empty() {
            out.push_str(&format!("**DOI:** {}\n", doi));
        }
    }
    if let Some(ref pages) = entry.page_range {
        if !pages.is_empty() {
            out.push_str(&format!("**Pages:** {}\n", pages));
        }
    }
    if let Some(ref issn) = entry.issn {
        if !issn.is_empty() {
            out.push_str(&format!("**ISSN:** {}\n", issn));
        }
    }
    if let Some(ref id) = entry.identifier {
        if !id.is_empty() {
            out.push_str(&format!("**Identifier:** {}\n", id));
        }
    }

    if let Some(ref desc) = entry.description {
        if !desc.is_empty() {
            let cleaned = strip_html_tags(desc);
            out.push_str("\n**Abstract:**\n\n");
            out.push_str(&cleaned);
            out.push('\n');
        }
    }

    out.trim_end().to_string()
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn entry_title_or_fallback(entry: &SearchEntry) -> &str {
    if entry.title.is_empty() {
        "(untitled)"
    } else {
        &entry.title
    }
}

fn truncate_at(s: &str, max: usize) -> &str {
    if s.len() <= max {
        s
    } else {
        let mut end = max;
        while end > 0 && !s.is_char_boundary(end) {
            end -= 1;
        }
        &s[..end]
    }
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

    #[test]
    fn format_search_results() {
        let resp = SearchResponse {
            total_results: "2".into(),
            start_index: "1".into(),
            items_per_page: "25".into(),
            entry: vec![
                SearchEntry {
                    title: "First paper".into(),
                    creator: Some("Smith J".into()),
                    doi: Some("10.1/a".into()),
                    publication_name: Some("Nature".into()),
                    volume: Some("1".into()),
                    cover_date: Some("2024-01-01".into()),
                    ..Default::default()
                },
                SearchEntry {
                    title: "Second paper".into(),
                    ..Default::default()
                },
            ],
            error: None,
        };
        let rendered = format_search(&resp);
        assert!(rendered.contains("**2** results"));
        assert!(rendered.contains("First paper"));
        assert!(rendered.contains("Smith J"));
        assert!(rendered.contains("Nature"));
    }

    #[test]
    fn format_search_empty() {
        let resp = SearchResponse {
            total_results: "0".into(),
            start_index: "1".into(),
            items_per_page: "25".into(),
            entry: vec![],
            error: None,
        };
        let rendered = format_search(&resp);
        assert!(rendered.contains("No entries"));
    }

    #[test]
    fn format_search_error() {
        let resp = SearchResponse {
            total_results: "0".into(),
            start_index: "1".into(),
            items_per_page: "25".into(),
            entry: vec![],
            error: Some(crate::types::ErrorPayload {
                status: Some("401".into()),
                detail: Some("Invalid API key".into()),
            }),
        };
        let rendered = format_search(&resp);
        assert!(rendered.contains("Invalid API key"));
    }

    #[test]
    fn format_retrieval_full() {
        let entry = SearchEntry {
            title: "Full Article".into(),
            creator: Some("Doe J".into()),
            doi: Some("10.1/x".into()),
            publication_name: Some("Science".into()),
            cover_date: Some("2023-06-15".into()),
            description: Some("The <i>full</i> abstract.".into()),
            ..Default::default()
        };
        let rendered = format_retrieval(&entry);
        assert!(rendered.contains("Full Article"));
        assert!(rendered.contains("Doe J"));
        assert!(rendered.contains("Science"));
        assert!(rendered.contains("full abstract"));
    }
}
