//! Render OpenAlex API responses into clean, LLM-friendly Markdown.

use crate::types::*;

// ===========================================================================
// Works list
// ===========================================================================

/// Format a list of works into readable Markdown previews.
pub fn format_works(resp: &ListResponse<Work>) -> String {
    let mut out = String::with_capacity(8192);

    out.push_str(&format!("**{}** works found", resp.meta.count));
    let n = resp.results.len();
    if n > 0 {
        out.push_str(&format!(" (showing {})\n\n", n));
    } else {
        out.push_str("\n\nNo works returned.\n");
        return out;
    }

    for (i, work) in resp.results.iter().enumerate() {
        format_work_preview(i + 1, work, &mut out);
        out.push('\n');
    }

    // Pagination hint.
    if let Some(ref cursor) = resp.meta.next_cursor {
        out.push_str(&format!("_Next cursor:_ `{cursor}`\n"));
    }
    // Cost hint.
    if let Some(cost) = resp.meta.cost_usd {
        if cost > 0.0 {
            out.push_str(&format!("_API cost: ${:.4}_\n", cost));
        }
    }

    out.trim_end().to_string()
}

fn format_work_preview(idx: usize, work: &Work, out: &mut String) {
    out.push_str(&format!("### {}. ", idx));
    if let Some(ref doi) = work.doi {
        out.push_str(&format!("[{}]({})\n", work.title_or_name().unwrap_or("(untitled)"), doi));
    } else {
        out.push_str(&format!(
            "{}\n",
            work.title_or_name().unwrap_or("(untitled)")
        ));
    }

    // ID + year + type
    let mut meta_parts = Vec::new();
    meta_parts.push(work.id.clone());
    if let Some(y) = work.publication_year {
        meta_parts.push(y.to_string());
    }
    if let Some(ref t) = work.type_ {
        meta_parts.push(t.clone());
    }
    meta_parts.push(format!("cited by {}", work.cited_by_count));
    out.push_str(&format!("`{}`\n", meta_parts.join(" · ")));

    // Authors (first 5)
    let author_names: Vec<String> = work
        .authorships
        .iter()
        .take(5)
        .map(|a| {
            a.author
                .display_name
                .clone()
                .unwrap_or_else(|| "(unknown)".into())
        })
        .collect();
    if !author_names.is_empty() {
        let suffix = if work.authorships.len() > 5 {
            format!(", +{} more", work.authorships.len() - 5)
        } else {
            String::new()
        };
        out.push_str(&format!("**Authors:** {}{}\n", author_names.join(", "), suffix));
    }

    // Venue
    if let Some(ref loc) = work.primary_location {
        if let Some(ref src) = loc.source {
            if let Some(ref name) = src.display_name {
                out.push_str(&format!("**Source:** {}\n", name));
            }
        }
    }

    // OA status
    if work.open_access.is_oa {
        out.push_str(&format!(
            "**OA:** {}{}\n",
            work.open_access.oa_status.as_deref().unwrap_or("yes"),
            work.open_access
                .oa_url
                .as_ref()
                .map(|u| format!(" — [full text]({})", u))
                .unwrap_or_default(),
        ));
    }

    // Topics (first 3)
    if !work.topics.is_empty() {
        let topics: Vec<String> = work
            .topics
            .iter()
            .take(3)
            .map(|t| t.display_name.clone())
            .collect();
        out.push_str(&format!("**Topics:** {}\n", topics.join("; ")));
    }

    // Abstract (truncated)
    if let Some(abs) = work.abstract_text() {
        let truncated = if abs.len() > 300 {
            format!("{}…", &abs[..300])
        } else {
            abs
        };
        out.push_str(&format!("**Abstract:** {}\n", truncated));
    }
}

// ===========================================================================
// Single work (full detail)
// ===========================================================================

/// Format a single work with full detail.
pub fn format_work_detail(work: &Work) -> String {
    let mut out = String::with_capacity(8192);
    format_work_preview(0, work, &mut out);

    // External IDs
    let mut ids = Vec::new();
    if let Some(ref d) = work.ids.doi {
        ids.push(format!("DOI: {}", d));
    }
    if let Some(ref p) = work.ids.pmid {
        ids.push(format!("PMID: {}", p));
    }
    if let Some(ref p) = work.ids.pmcid {
        ids.push(format!("PMCID: {}", p));
    }
    if !ids.is_empty() {
        out.push_str(&format!("\n**IDs:** {}\n", ids.join(" · ")));
    }

    // All locations
    if !work.locations.is_empty() {
        out.push_str("\n**Locations:**\n");
        for loc in work.locations.iter().take(10) {
            if let Some(ref src) = loc.source {
                let name = src.display_name.as_deref().unwrap_or("(unknown)");
                let oa = if loc.is_oa { " [OA]" } else { "" };
                out.push_str(&format!("- {}{}\n", name, oa));
            }
        }
    }

    // MeSH terms
    if !work.mesh.is_empty() {
        let mesh: Vec<String> = work
            .mesh
            .iter()
            .map(|m| m.descriptor_name.clone().unwrap_or_default())
            .collect();
        out.push_str(&format!("\n**MeSH:** {}\n", mesh.join(", ")));
    }

    // Keywords
    if !work.keywords.is_empty() {
        let kw: Vec<String> = work
            .keywords
            .iter()
            .take(10)
            .map(|k| k.display_name.clone())
            .collect();
        out.push_str(&format!("\n**Keywords:** {}\n", kw.join(", ")));
    }

    // Citation counts by year (last 5)
    if !work.counts_by_year.is_empty() {
        let recent: Vec<String> = work
            .counts_by_year
            .iter()
            .take(5)
            .map(|c| format!("{}: {}", c.year, c.cited_by_count))
            .collect();
        out.push_str(&format!("\n**Citations by year:** {}\n", recent.join(", ")));
    }

    // References count
    out.push_str(&format!(
        "\n**References:** {} | **Related works:** {}\n",
        work.referenced_works.len(),
        work.related_works.len()
    ));

    out.trim_end().to_string()
}

// ===========================================================================
// Authors list
// ===========================================================================

/// Format a list of authors into readable Markdown.
pub fn format_authors(resp: &ListResponse<Author>) -> String {
    let mut out = String::with_capacity(4096);
    out.push_str(&format!("**{}** authors found", resp.meta.count));
    let n = resp.results.len();
    if n > 0 {
        out.push_str(&format!(" (showing {})\n\n", n));
    } else {
        out.push_str("\n\nNo authors returned.\n");
        return out;
    }

    for (i, author) in resp.results.iter().enumerate() {
        out.push_str(&format!(
            "{}. **{}** — {} works, {} citations",
            i + 1,
            author.display_name,
            author.works_count,
            author.cited_by_count
        ));
        if let Some(ref orcid) = author.orcid {
            out.push_str(&format!(" | ORCID: {}", orcid));
        }
        if let Some(ref stats) = author.summary_stats {
            if let Some(h) = stats.h_index {
                out.push_str(&format!(" | h-index: {}", h));
            }
        }
        if !author.last_known_institutions.is_empty() {
            let insts: Vec<String> = author
                .last_known_institutions
                .iter()
                .filter_map(|i| i.display_name.clone())
                .collect();
            out.push_str(&format!(" | {}", insts.join(", ")));
        }
        out.push('\n');
        out.push_str(&format!("   _ID: {}_\n", author.id));
    }

    out.trim_end().to_string()
}

// ===========================================================================
// Autocomplete
// ===========================================================================

/// Format autocomplete suggestions into readable Markdown.
pub fn format_autocomplete(resp: &AutocompleteResponse) -> String {
    let mut out = String::with_capacity(2048);
    let n = resp.results.len();
    if n == 0 {
        return "No suggestions found.".into();
    }
    out.push_str(&format!("**{}** suggestions:\n\n", n));
    for (i, r) in resp.results.iter().enumerate() {
        out.push_str(&format!(
            "{}. **{}**",
            i + 1,
            r.display_name.as_deref().unwrap_or("(unknown)")
        ));
        if let Some(ref id) = r.id {
            out.push_str(&format!(" (`{}`)", id));
        }
        if let Some(score) = r.relevance_score {
            out.push_str(&format!(" — score: {:.2}", score));
        }
        if let Some(wc) = r.work_count {
            out.push_str(&format!(" — {} works", wc));
        }
        out.push('\n');
    }
    out.trim_end().to_string()
}

// ===========================================================================
// Group-by aggregation
// ===========================================================================

/// Format group-by results into a Markdown table.
pub fn format_group_by(group_by: &[GroupByEntry]) -> String {
    if group_by.is_empty() {
        return "No aggregation results.".into();
    }
    let mut out = String::with_capacity(1024);
    out.push_str("| Key | Count |\n|-----|-------|\n");
    for entry in group_by {
        let label = entry
            .key_display_name
            .as_deref()
            .or(entry.key_name.as_deref())
            .unwrap_or(&entry.key);
        out.push_str(&format!("| {} | {} |\n", label, entry.count));
    }
    out.trim_end().to_string()
}

// ===========================================================================
// Tests (no network)
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::*;

    fn sample_work() -> Work {
        Work {
            id: "https://openalex.org/W2741809807".into(),
            doi: Some("https://doi.org/10.7717/peerj.4375".into()),
            ids: WorkIds {
                doi: Some("https://doi.org/10.7717/peerj.4375".into()),
                pmid: Some("https://pubmed.ncbi.nlm.nih.gov/29456894".into()),
                ..Default::default()
            },
            display_name: Some("The state of OA".into()),
            publication_year: Some(2018),
            type_: Some("article".into()),
            cited_by_count: 100,
            open_access: OpenAccess {
                is_oa: true,
                oa_status: Some("gold".into()),
                oa_url: Some("https://example.com/paper.pdf".into()),
                ..Default::default()
            },
            authorships: vec![
                Authorship {
                    author_position: Some("first".into()),
                    author: DehydratedAuthor {
                        id: Some("https://openalex.org/A1".into()),
                        display_name: Some("Jane Doe".into()),
                        ..Default::default()
                    },
                    ..Default::default()
                },
                Authorship {
                    author_position: Some("last".into()),
                    author: DehydratedAuthor {
                        display_name: Some("John Smith".into()),
                        ..Default::default()
                    },
                    ..Default::default()
                },
            ],
            topics: vec![TopicAssignment {
                id: "https://openalex.org/T1".into(),
                display_name: "Open Access".into(),
                ..Default::default()
            }],
            primary_location: Some(Location {
                source: Some(DehydratedSource {
                    display_name: Some("PeerJ".into()),
                    ..Default::default()
                }),
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    #[test]
    fn format_works_list_nonempty() {
        let resp = ListResponse {
            meta: Meta {
                count: 100,
                per_page: Some(2),
                next_cursor: Some("abc".into()),
                ..Default::default()
            },
            results: vec![sample_work()],
            group_by: vec![],
        };
        let md = format_works(&resp);
        assert!(md.contains("100"));
        assert!(md.contains("The state of OA"));
        assert!(md.contains("Jane Doe"));
        assert!(md.contains("Next cursor"));
    }

    #[test]
    fn format_works_empty() {
        let resp = ListResponse::<Work> {
            meta: Meta { count: 0, ..Default::default() },
            results: vec![],
            group_by: vec![],
        };
        let md = format_works(&resp);
        assert!(md.contains("No works returned"));
    }

    #[test]
    fn format_work_detail_includes_ids() {
        let work = sample_work();
        let md = format_work_detail(&work);
        assert!(md.contains("The state of OA"));
        assert!(md.contains("DOI:"));
        assert!(md.contains("PeerJ"));
        assert!(md.contains("Open Access"));
    }

    #[test]
    fn format_authors_nonempty() {
        let resp = ListResponse {
            meta: Meta { count: 1, ..Default::default() },
            results: vec![Author {
                id: "https://openalex.org/A1".into(),
                display_name: "Jane Doe".into(),
                orcid: Some("https://orcid.org/0000-0001-2345-6789".into()),
                works_count: 50,
                cited_by_count: 1000,
                summary_stats: Some(AuthorSummaryStats {
                    h_index: Some(15),
                    ..Default::default()
                }),
                ..Default::default()
            }],
            group_by: vec![],
        };
        let md = format_authors(&resp);
        assert!(md.contains("Jane Doe"));
        assert!(md.contains("50 works"));
        assert!(md.contains("h-index: 15"));
        assert!(md.contains("ORCID"));
    }

    #[test]
    fn format_autocomplete_nonempty() {
        let resp = AutocompleteResponse {
            meta: Meta { count: 2, ..Default::default() },
            results: vec![AutocompleteResult {
                id: Some("https://openalex.org/W1".into()),
                display_name: Some("Machine Learning".into()),
                relevance_score: Some(0.95),
                work_count: Some(5000),
                ..Default::default()
            }],
        };
        let md = format_autocomplete(&resp);
        assert!(md.contains("Machine Learning"));
        assert!(md.contains("0.95"));
        assert!(md.contains("5000 works"));
    }

    #[test]
    fn format_autocomplete_empty() {
        let resp = AutocompleteResponse::default();
        let md = format_autocomplete(&resp);
        assert_eq!(md, "No suggestions found.");
    }

    #[test]
    fn format_group_by_table() {
        let entries = vec![
            GroupByEntry {
                key: "article".into(),
                key_display_name: Some("Article".into()),
                count: 80,
                ..Default::default()
            },
            GroupByEntry {
                key: "book".into(),
                key_display_name: Some("Book".into()),
                count: 20,
                ..Default::default()
            },
        ];
        let md = format_group_by(&entries);
        assert!(md.contains("| Key | Count |"));
        assert!(md.contains("Article | 80"));
        assert!(md.contains("Book | 20"));
    }

    #[test]
    fn format_group_by_empty() {
        let md = format_group_by(&[]);
        assert_eq!(md, "No aggregation results.");
    }

    #[test]
    fn abstract_truncation() {
        let mut work = sample_work();
        let long_text = "word ".repeat(100);
        let mut inv = std::collections::BTreeMap::new();
        for (i, word) in long_text.split_whitespace().enumerate() {
            inv.entry(word.to_string()).or_insert_with(Vec::new).push(i as u32);
        }
        work.abstract_inverted_index = Some(inv);
        let md = format_work_detail(&work);
        assert!(md.contains("…") || md.contains("..."));
    }
}
