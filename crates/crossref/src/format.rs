//! Render Crossref API responses into clean, LLM-friendly Markdown.

use crate::types::*;

// ===========================================================================
// Works list
// ===========================================================================

/// Format a works list response into readable Markdown with article previews.
pub fn format_works(resp: &WorksListResponse) -> String {
    let msg = &resp.message;
    let total = msg.total_results;
    let items = &msg.items;

    let mut out = String::with_capacity(8192);
    out.push_str(&format!("**{total}** results found"));
    if items.is_empty() {
        out.push_str("\n\nNo entries returned.\n");
        return out;
    }
    out.push_str(&format!(" (showing {})\n\n", items.len()));

    for (i, work) in items.iter().enumerate() {
        format_work_preview(i + 1, work, &mut out);
        out.push('\n');
    }

    if let Some(ref cursor) = msg.next_cursor {
        out.push_str(&format!("_Next cursor:_ `{cursor}`\n\n"));
    }

    out.trim_end().to_string()
}

// ---------------------------------------------------------------------------
// Single work (full detail)
// ---------------------------------------------------------------------------

/// Format a single work with full metadata.
pub fn format_work(work: &Work) -> String {
    let mut out = String::with_capacity(8192);
    format_work_full(work, &mut out);
    out.trim_end().to_string()
}

// ---------------------------------------------------------------------------
// Funder list
// ---------------------------------------------------------------------------

/// Format a funders list response.
pub fn format_funders(resp: &FundersListResponse) -> String {
    let msg = &resp.message;
    let items = &msg.items;
    let mut out = String::with_capacity(4096);
    out.push_str(&format!("**{}** funders", msg.total_results));
    if items.is_empty() {
        out.push_str("\n\nNo funders found.\n");
        return out;
    }
    out.push_str(&format!(", showing {}\n\n", items.len()));

    for (i, f) in items.iter().enumerate() {
        out.push_str(&format!("{}. **{}**", i + 1, f.name));
        if !f.id.is_empty() {
            out.push_str(&format!(" — `{}`", f.id));
        }
        out.push('\n');
    }
    out.trim_end().to_string()
}

// ---------------------------------------------------------------------------
// Member list
// ---------------------------------------------------------------------------

/// Format a members list response.
pub fn format_members(resp: &MembersListResponse) -> String {
    let msg = &resp.message;
    let items = &msg.items;
    let mut out = String::with_capacity(4096);
    out.push_str(&format!("**{}** members", msg.total_results));
    if items.is_empty() {
        out.push_str("\n\nNo members found.\n");
        return out;
    }
    out.push_str(&format!(", showing {}\n\n", items.len()));

    for (i, m) in items.iter().enumerate() {
        out.push_str(&format!(
            "{}. **{}** (ID: {}, {} DOIs)\n",
            i + 1,
            if m.primary_name.is_empty() {
                "(unknown)"
            } else {
                &m.primary_name
            },
            m.id,
            m.total_doi_count
        ));
    }
    out.trim_end().to_string()
}

// ---------------------------------------------------------------------------
// Journals list
// ---------------------------------------------------------------------------

/// Format a journals list response.
pub fn format_journals(resp: &JournalsListResponse) -> String {
    let msg = &resp.message;
    let items = &msg.items;
    let mut out = String::with_capacity(4096);
    out.push_str(&format!("**{}** journals", msg.total_results));
    if items.is_empty() {
        out.push_str("\n\nNo journals found.\n");
        return out;
    }
    out.push_str(&format!(", showing {}\n\n", items.len()));

    for (i, j) in items.iter().enumerate() {
        out.push_str(&format!(
            "{}. **{}** — {}",
            i + 1,
            if j.title.is_empty() {
                "(unknown)"
            } else {
                &j.title
            },
            j.issn.join(", ")
        ));
        out.push('\n');
    }
    out.trim_end().to_string()
}

// ---------------------------------------------------------------------------
// Agency
// ---------------------------------------------------------------------------

/// Format a DOI agency response.
pub fn format_agency(resp: &AgencyResponse) -> String {
    match resp.message.as_ref() {
        Some(info) => format!(
            "DOI **{}** is registered with **{}** ({}).",
            info.doi, info.agency.label, info.agency.id
        ),
        None => "No agency information returned.".to_string(),
    }
}

// ===========================================================================
// Per-entry formatters
// ===========================================================================

fn format_work_preview(i: usize, work: &Work, out: &mut String) {
    out.push_str(&format!("### {}. {}\n", i, work.title_str()));

    // DOI.
    if !work.doi.is_empty() {
        out.push_str(&format!("**DOI:** {}\n", work.doi));
    }

    // Authors (truncated if long).
    if !work.author.is_empty() {
        let display = if work.author.len() > 4 {
            format!(
                "{} et al. ({} authors)",
                work.author[0].display(),
                work.author.len()
            )
        } else {
            work.author
                .iter()
                .map(|a| a.display())
                .collect::<Vec<_>>()
                .join(", ")
        };
        out.push_str(&format!("**Authors:** {display}\n"));
    }

    // Journal.
    if let Some(j) = work.journal_str() {
        out.push_str(&format!("**Journal:** {j}\n"));
    }

    // Year.
    if let Some(y) = work.year() {
        out.push_str(&format!("**Year:** {y}\n"));
    }

    // Type.
    if !work.r#type.is_empty() {
        out.push_str(&format!("**Type:** {}\n", work.r#type));
    }

    // Cited by.
    if work.is_referenced_by_count > 0 {
        out.push_str(&format!("**Cited by:** {}\n", work.is_referenced_by_count));
    }

    // Abstract (truncated preview).
    if let Some(ref abs) = work.abstract_text {
        let cleaned = strip_tags(abs);
        if !cleaned.is_empty() {
            let preview = truncate_at(&cleaned, 400);
            out.push_str(&format!("\n> {preview}\n"));
            if cleaned.len() > 400 {
                out.push_str("> ...\n");
            }
        }
    }

    out.push('\n');
}

fn format_work_full(work: &Work, out: &mut String) {
    out.push_str(&format!("## {}\n\n", work.title_str()));

    if !work.doi.is_empty() {
        out.push_str(&format!("**DOI:** {}\n", work.doi));
    }
    if !work.url.is_empty() {
        out.push_str(&format!("**URL:** {}\n", work.url));
    }

    // Full author list.
    if !work.author.is_empty() {
        out.push_str("**Authors:** ");
        let parts: Vec<String> = work
            .author
            .iter()
            .map(|a| {
                let mut s = a.display();
                if let Some(ref orc) = a.orcid {
                    let id = orc.rsplit('/').next().unwrap_or(orc);
                    s.push_str(&format!(" [ORCID: {id}]"));
                }
                if let Some(first) = a.affiliation.first() {
                    if !first.name.is_empty() {
                        s.push_str(&format!(" ({})", first.name));
                    }
                }
                s
            })
            .collect();
        out.push_str(&parts.join(", "));
        out.push('\n');
    }

    // Journal info.
    if let Some(j) = work.journal_str() {
        out.push_str(&format!("**Journal:** {j}\n"));
    }
    if let Some(ref st) = work.short_container_title.first() {
        if !st.is_empty() {
            out.push_str(&format!("**Journal abbr:** {st}\n"));
        }
    }
    if !work.issn.is_empty() {
        out.push_str(&format!("**ISSN:** {}\n", work.issn.join(", ")));
    }
    if !work.isbn.is_empty() {
        out.push_str(&format!("**ISBN:** {}\n", work.isbn.join(", ")));
    }

    if !work.publisher.is_empty() {
        out.push_str(&format!("**Publisher:** {}\n", work.publisher));
    }
    if !work.volume.is_empty() {
        out.push_str(&format!("**Volume:** {}\n", work.volume));
    }
    if !work.issue.is_empty() {
        out.push_str(&format!("**Issue:** {}\n", work.issue));
    }
    if !work.page.is_empty() {
        out.push_str(&format!("**Pages:** {}\n", work.page));
    } else if !work.article_number.is_empty() {
        out.push_str(&format!("**Article number:** {}\n", work.article_number));
    }

    // Dates.
    if let Some(ref dp) = work.issued {
        if let Some(y) = dp.year() {
            out.push_str(&format!("**Year:** {y}\n"));
        }
    }
    if let Some(ref dp) = work.published_online {
        if let Some(y) = dp.year() {
            out.push_str(&format!("**Published online:** {y}\n"));
        }
    }

    if !work.r#type.is_empty() {
        out.push_str(&format!("**Type:** {}\n", work.r#type));
    }
    if !work.language.is_empty() {
        out.push_str(&format!("**Language:** {}\n", work.language));
    }

    // Counts.
    out.push_str(&format!("**Cited by:** {}\n", work.is_referenced_by_count));
    out.push_str(&format!("**References count:** {}\n", work.references_count));

    // Subjects.
    if !work.subject.is_empty() {
        out.push_str(&format!("**Subjects:** {}\n", work.subject.join(", ")));
    }

    // Funders.
    if !work.funder.is_empty() {
        out.push_str("**Funders:**\n");
        for f in &work.funder {
            out.push_str(&format!("- {}", f.name));
            if let Some(ref d) = f.doi {
                out.push_str(&format!(" ({d})"));
            }
            if !f.award.is_empty() {
                out.push_str(&format!(" — {}", f.award.join(", ")));
            }
            out.push('\n');
        }
    }

    // License.
    if !work.license.is_empty() {
        for l in &work.license {
            if !l.url.is_empty() {
                out.push_str(&format!("**License:** {}\n", l.url));
            }
        }
    }

    // Full abstract.
    if let Some(ref abs) = work.abstract_text {
        let cleaned = strip_tags(abs);
        if !cleaned.is_empty() {
            out.push_str("\n**Abstract:**\n\n");
            out.push_str(&cleaned);
            out.push('\n');
        }
    }

    out.push('\n');
}

// ===========================================================================
// Helpers
// ===========================================================================

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

fn strip_tags(s: &str) -> String {
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
    // Collapse extra whitespace.
    while out.contains("  ") {
        out = out.replace("  ", " ");
    }
    out.trim().to_string()
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_work() -> Work {
        Work {
            doi: "10.1037/0003-066x.59.1.29".into(),
            title: vec!["Toward a science of ego depletion".into()],
            container_title: vec!["American Psychologist".into()],
            author: vec![Author {
                given: "Roy F".into(),
                family: "Baumeister".into(),
                sequence: "first".into(),
                orcid: Some("https://orcid.org/0000-0002-1234-5678".into()),
                affiliation: vec![Affiliation {
                    name: "FSU".into(),
                }],
            }],
            issued: Some(DateParts {
                date_parts: vec![vec![Some(2004), Some(7)]],
                ..Default::default()
            }),
            volume: "59".into(),
            issue: "1".into(),
            page: "29-36".into(),
            r#type: "journal-article".into(),
            is_referenced_by_count: 5000,
            references_count: 120,
            publisher: "APA".into(),
            abstract_text: Some(
                "<jats:p>Self-regulation involves ego depletion.</jats:p>".into(),
            ),
            ..Default::default()
        }
    }

    #[test]
    fn format_works_basic() {
        let resp = WorksListResponse {
            status: "ok".into(),
            message_type: "work-list".into(),
            message_version: "1.0.0".into(),
            message: ListMessage {
                total_results: 42,
                items_per_page: Some(1),
                query: serde_json::Value::Null,
                next_cursor: Some("AoE/cursor==".into()),
                items: vec![sample_work()],
            },
        };
        let rendered = format_works(&resp);
        assert!(rendered.contains("**42** results"));
        assert!(rendered.contains("Toward a science of ego depletion"));
        assert!(rendered.contains("DOI:** 10.1037"));
        assert!(rendered.contains("AoE/cursor=="));
    }

    #[test]
    fn format_works_empty() {
        let resp = WorksListResponse {
            status: "ok".into(),
            message_type: "work-list".into(),
            message_version: "1.0.0".into(),
            message: ListMessage::default(),
        };
        let rendered = format_works(&resp);
        assert!(rendered.contains("No entries"));
    }

    #[test]
    fn format_single_work() {
        let rendered = format_work(&sample_work());
        assert!(rendered.contains("## Toward a science of ego depletion"));
        assert!(rendered.contains("**Abstract:**"));
        assert!(rendered.contains("Self-regulation"));
        assert!(rendered.contains("Cited by:** 5000"));
    }
}
