//! Render Europe PMC API responses into clean, LLM-friendly Markdown.

use crate::types::*;

// ===========================================================================
// Search results
// ===========================================================================

/// Format a Europe PMC [`SearchResponse`] into readable Markdown with article
/// previews.
pub fn format_search(resp: &SearchResponse) -> String {
    let mut out = String::with_capacity(8192);

    out.push_str(&format!("**{}** results found", resp.hit_count));
    let n = resp.result_list.results.len();
    if n > 0 {
        out.push_str(&format!(" (showing {})\n\n", n));
    } else {
        out.push_str("\n\nNo entries returned.\n");
        return out;
    }

    for (i, result) in resp.result_list.results.iter().enumerate() {
        format_result_preview(i + 1, result, &mut out);
        out.push('\n');
    }

    // Include pagination hint.
    if let Some(ref cursor) = resp.next_cursor_mark {
        out.push_str(&format!(
            "_Next page cursor:_ `{cursor}`\n\n"
        ));
    }

    out.trim_end().to_string()
}

// ---------------------------------------------------------------------------
// Article (full detail)
// ---------------------------------------------------------------------------

/// Format a single article result with full detail (abstract, MeSH, etc.).
pub fn format_article(result: &SearchResult) -> String {
    let mut out = String::with_capacity(8192);
    format_result_full(result, &mut out);
    out.trim_end().to_string()
}

// ---------------------------------------------------------------------------
// References
// ---------------------------------------------------------------------------

/// Format a references response into readable Markdown.
pub fn format_references(resp: &ReferencesResponse) -> String {
    let mut out = String::with_capacity(4096);
    out.push_str(&format!("**{}** references", resp.hit_count));

    let refs: &[ReferenceEntry] = resp
        .reference_list
        .as_ref()
        .map(|rl| rl.references.as_slice())
        .unwrap_or(&[]);

    if refs.is_empty() {
        out.push_str("\n\nNo references found.\n");
        return out;
    }
    out.push_str(&format!(", showing {}\n\n", refs.len()));

    for (i, r) in refs.iter().enumerate() {
        format_reference_entry(i + 1, r, &mut out);
        out.push('\n');
    }

    out.trim_end().to_string()
}

// ---------------------------------------------------------------------------
// Citations
// ---------------------------------------------------------------------------

/// Format a citations response into readable Markdown.
pub fn format_citations(resp: &CitationsResponse) -> String {
    let mut out = String::with_capacity(4096);
    out.push_str(&format!("**{}** citations", resp.hit_count));

    let cites: &[CitationEntry] = resp
        .citation_list
        .as_ref()
        .map(|cl| cl.citations.as_slice())
        .unwrap_or(&[]);

    if cites.is_empty() {
        out.push_str("\n\nNo citations found.\n");
        return out;
    }
    out.push_str(&format!(", showing {}\n\n", cites.len()));

    for (i, c) in cites.iter().enumerate() {
        format_citation_entry(i + 1, c, &mut out);
        out.push('\n');
    }

    out.trim_end().to_string()
}

// ---------------------------------------------------------------------------
// Profile
// ---------------------------------------------------------------------------

/// Format a profile response into readable Markdown.
pub fn format_profile(resp: &ProfileResponse) -> String {
    let mut out = String::with_capacity(2048);

    if let Some(ref pl) = resp.profile_list {
        if let Some(ref pts) = pl.pub_types {
            out.push_str("### By publication type\n\n");
            out.push_str("| Type | Count |\n|------|-------|\n");
            for pt in pts {
                out.push_str(&format!("| {} | {} |\n", pt.name, pt.count));
            }
            out.push('\n');
        }
        if let Some(ref srcs) = pl.sources {
            out.push_str("### By source\n\n");
            out.push_str("| Source | Count |\n|--------|-------|\n");
            for s in srcs {
                if s.count > 0 {
                    out.push_str(&format!("| {} | {} |\n", s.name, s.count));
                }
            }
            out.push('\n');
        }
    }

    if out.is_empty() {
        out.push_str("No profile data returned.\n");
    }

    out.trim_end().to_string()
}

// ===========================================================================
// Per-entry formatters
// ===========================================================================

fn format_result_preview(i: usize, result: &SearchResult, out: &mut String) {
    out.push_str(&format!(
        "### {}. {}\n",
        i,
        if result.title.is_empty() {
            "(untitled)"
        } else {
            &result.title
        }
    ));

    // Source + ID
    out.push_str(&format!("**Source:** {} {}\n", result.source, result.id));

    // Authors
    if let Some(ref astr) = result.author_string {
        if !astr.is_empty() {
            let display = if astr.matches(',').count() > 4 {
                format!("{} et al.", astr.split(',').next().unwrap_or(""))
            } else {
                astr.clone()
            };
            out.push_str(&format!("**Authors:** {display}\n"));
        }
    }

    // Journal
    let journal_name = result
        .journal_info
        .as_ref()
        .and_then(|ji| ji.journal.as_ref())
        .and_then(|j| j.title.as_deref())
        .or(result.journal_title.as_deref())
        .unwrap_or("");
    if !journal_name.is_empty() {
        out.push_str(&format!("**Journal:** {journal_name}\n"));
    }

    // Year
    let year = result
        .journal_info
        .as_ref()
        .and_then(|ji| ji.year_of_publication.map(|y| y.to_string()))
        .or_else(|| result.pub_year.clone());
    if let Some(y) = year {
        out.push_str(&format!("**Year:** {y}\n"));
    }

    // DOI
    if let Some(ref doi) = result.doi {
        if !doi.is_empty() {
            out.push_str(&format!("**DOI:** {doi}\n"));
        }
    }

    // Cited by
    if let Some(c) = result.cited_by_count {
        if c > 0 {
            out.push_str(&format!("**Cited by:** {c}\n"));
        }
    }

    // Abstract (truncated for preview)
    if let Some(ref abs) = result.abstract_text {
        if !abs.is_empty() {
            let cleaned = strip_html_tags(abs);
            let preview = truncate_at(&cleaned, 400);
            out.push_str(&format!("\n> {preview}\n"));
            if cleaned.len() > 400 {
                out.push_str("> ...\n");
            }
        }
    }

    out.push('\n');
}

fn format_result_full(result: &SearchResult, out: &mut String) {
    out.push_str(&format!(
        "## {}\n\n",
        if result.title.is_empty() {
            "(untitled)"
        } else {
            &result.title
        }
    ));

    // Source + ID
    out.push_str(&format!("**Source:** {} {}\n", result.source, result.id));

    // Full author list
    if let Some(ref al) = result.author_list {
        if !al.authors.is_empty() {
            out.push_str("**Authors:** ");
            let parts: Vec<String> = al
                .authors
                .iter()
                .map(|a| {
                    let mut s = a.full_name.clone();
                    if let Some(ref aff) = a.affiliation_details {
                        if let Some(first) = aff.affiliations.first() {
                            if !first.affiliation.is_empty() {
                                s.push_str(&format!(" ({})", strip_html_tags(&first.affiliation)));
                            }
                        }
                    }
                    if let Some(ref id) = a.author_id {
                        if id.id_type.eq_ignore_ascii_case("ORCID") {
                            s.push_str(&format!(" [ORCID: {}]", id.value));
                        }
                    }
                    s
                })
                .collect();
            out.push_str(&parts.join(", "));
            out.push('\n');
        }
    } else if let Some(ref astr) = result.author_string {
        if !astr.is_empty() {
            out.push_str(&format!("**Authors:** {astr}\n"));
        }
    }

    // Journal info
    if let Some(ref ji) = result.journal_info {
        if let Some(ref j) = ji.journal {
            if let Some(ref title) = j.title {
                if !title.is_empty() {
                    out.push_str(&format!("**Journal:** {title}\n"));
                }
            }
            if let Some(ref issn) = j.issn {
                if !issn.is_empty() {
                    out.push_str(&format!("**ISSN:** {issn}\n"));
                }
            }
            if let Some(ref essn) = j.essn {
                if !essn.is_empty() {
                    out.push_str(&format!("**ESSN:** {essn}\n"));
                }
            }
        }
        if let Some(ref v) = ji.volume {
            if !v.is_empty() {
                out.push_str(&format!("**Volume:** {v}\n"));
            }
        }
        if let Some(ref iss) = ji.issue {
            if !iss.is_empty() {
                out.push_str(&format!("**Issue:** {iss}\n"));
            }
        }
        if let Some(y) = ji.year_of_publication {
            out.push_str(&format!("**Year:** {y}\n"));
        }
    } else {
        if let Some(ref name) = result.journal_title {
            if !name.is_empty() {
                out.push_str(&format!("**Journal:** {name}\n"));
            }
        }
        if let Some(ref y) = result.pub_year {
            out.push_str(&format!("**Year:** {y}\n"));
        }
        if let Some(ref v) = result.journal_volume {
            if !v.is_empty() {
                out.push_str(&format!("**Volume:** {v}\n"));
            }
        }
        if let Some(ref iss) = result.issue {
            if !iss.is_empty() {
                out.push_str(&format!("**Issue:** {iss}\n"));
            }
        }
    }

    // Pages
    if let Some(ref p) = result.page_info {
        if !p.is_empty() {
            out.push_str(&format!("**Pages:** {p}\n"));
        }
    }

    // DOI
    if let Some(ref doi) = result.doi {
        if !doi.is_empty() {
            out.push_str(&format!("**DOI:** {doi}\n"));
        }
    }

    // PMID
    if let Some(ref pmid) = result.pmid {
        if !pmid.is_empty() {
            out.push_str(&format!("**PMID:** {pmid}\n"));
        }
    }

    // Language
    if let Some(ref lang) = result.language {
        if !lang.is_empty() {
            out.push_str(&format!("**Language:** {lang}\n"));
        }
    }

    // Publication types
    if let Some(ref ptl) = result.pub_type_list {
        if !ptl.types.is_empty() {
            out.push_str(&format!("**Pub types:** {}\n", ptl.types.join(", ")));
        }
    } else if let Some(ref pt) = result.pub_type {
        if !pt.is_empty() {
            out.push_str(&format!("**Pub type:** {pt}\n"));
        }
    }

    // MeSH headings
    if let Some(ref mhl) = result.mesh_heading_list {
        if !mhl.headings.is_empty() {
            out.push_str("**MeSH terms:** ");
            let parts: Vec<String> = mhl
                .headings
                .iter()
                .map(|h| {
                    if h.major_topic.eq_ignore_ascii_case("Y") {
                        format!("*{}*", h.descriptor_name)
                    } else {
                        h.descriptor_name.clone()
                    }
                })
                .collect();
            out.push_str(&parts.join("; "));
            out.push('\n');
        }
    }

    // Cited by
    if let Some(c) = result.cited_by_count {
        if c > 0 {
            out.push_str(&format!("**Cited by:** {c}\n"));
        }
    }

    // Full abstract
    if let Some(ref abs) = result.abstract_text {
        if !abs.is_empty() {
            let cleaned = strip_html_tags(abs);
            out.push_str("\n**Abstract:**\n\n");
            out.push_str(&cleaned);
            out.push('\n');
        }
    }

    out.push('\n');
}

fn format_reference_entry(i: usize, r: &ReferenceEntry, out: &mut String) {
    out.push_str(&format!(
        "### {}. {}\n",
        i,
        r.title.as_deref().unwrap_or("(untitled)")
    ));
    if let Some(ref a) = r.author_string {
        if !a.is_empty() {
            out.push_str(&format!("**Authors:** {a}\n"));
        }
    }
    if let Some(ref j) = r.journal_abbreviation {
        if !j.is_empty() {
            out.push_str(&format!("**Journal:** {j}\n"));
        }
    }
    if let Some(y) = r.pub_year {
        out.push_str(&format!("**Year:** {y}\n"));
    }
    if let Some(ref v) = r.volume {
        if !v.is_empty() {
            out.push_str(&format!("**Volume:** {v}\n"));
        }
    }
    if let Some(ref iss) = r.issue {
        if !iss.is_empty() {
            out.push_str(&format!("**Issue:** {iss}\n"));
        }
    }
    if let Some(ref p) = r.page_info {
        if !p.is_empty() {
            out.push_str(&format!("**Pages:** {p}\n"));
        }
    }
    if let Some(ref id) = r.id {
        if !id.is_empty() {
            if let Some(ref src) = r.source {
                out.push_str(&format!("**Source:** {} {}\n", src, id));
            }
        }
    }
    out.push('\n');
}

fn format_citation_entry(i: usize, c: &CitationEntry, out: &mut String) {
    out.push_str(&format!(
        "### {}. {}\n",
        i,
        c.title.as_deref().unwrap_or("(untitled)")
    ));
    if let Some(ref a) = c.author_string {
        if !a.is_empty() {
            out.push_str(&format!("**Authors:** {a}\n"));
        }
    }
    if let Some(ref j) = c.journal_abbreviation {
        if !j.is_empty() {
            out.push_str(&format!("**Journal:** {j}\n"));
        }
    }
    if let Some(y) = c.pub_year {
        out.push_str(&format!("**Year:** {y}\n"));
    }
    if let Some(ref v) = c.volume {
        if !v.is_empty() {
            out.push_str(&format!("**Volume:** {v}\n"));
        }
    }
    if let Some(ref iss) = c.issue {
        if !iss.is_empty() {
            out.push_str(&format!("**Issue:** {iss}\n"));
        }
    }
    if let Some(ref p) = c.page_info {
        if !p.is_empty() {
            out.push_str(&format!("**Pages:** {p}\n"));
        }
    }
    if let Some(ref id) = c.id {
        if !id.is_empty() {
            if let Some(ref src) = c.source {
                out.push_str(&format!("**Source:** {} {}\n", src, id));
            }
        }
    }
    if let Some(n) = c.cited_by_count {
        if n > 0 {
            out.push_str(&format!("**Cited by:** {n}\n"));
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

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_result() -> SearchResult {
        SearchResult {
            id: "42541598".into(),
            source: "MED".into(),
            pmid: Some("42541598".into()),
            doi: Some("10.1007/s11033-026-12527-x".into()),
            title: "Mutant p53 drives cancer.".into(),
            author_string: Some("Fang Y, Zhang T, Ye Q.".into()),
            journal_title: Some("Mol Biol Rep".into()),
            pub_year: Some("2026".into()),
            cited_by_count: Some(5),
            ..Default::default()
        }
    }

    #[test]
    fn format_search_basic() {
        let resp = SearchResponse {
            version: "6.9".into(),
            hit_count: 42,
            next_cursor_mark: Some("AoIIcursor==".into()),
            request: RequestEcho::default(),
            result_list: ResultList {
                results: vec![sample_result()],
            },
        };
        let rendered = format_search(&resp);
        assert!(rendered.contains("**42** results"));
        assert!(rendered.contains("Mutant p53 drives cancer."));
        assert!(rendered.contains("Source:** MED 42541598"));
        assert!(rendered.contains("DOI:** 10.1007"));
        assert!(rendered.contains("AoIIcursor=="));
    }

    #[test]
    fn format_search_empty() {
        let resp = SearchResponse {
            version: "6.9".into(),
            hit_count: 0,
            next_cursor_mark: None,
            request: RequestEcho::default(),
            result_list: ResultList::default(),
        };
        let rendered = format_search(&resp);
        assert!(rendered.contains("No entries"));
    }

    #[test]
    fn format_article_with_abstract() {
        let result = SearchResult {
            abstract_text: Some("This is a <b>great</b> paper.".into()),
            ..sample_result()
        };
        let rendered = format_article(&result);
        assert!(rendered.contains("## Mutant p53 drives cancer."));
        assert!(rendered.contains("**Abstract:**"));
        assert!(rendered.contains("great paper."));
    }

    #[test]
    fn format_references_basic() {
        let resp = ReferencesResponse {
            version: "6.9".into(),
            hit_count: 19,
            request: ReferenceRequestEcho::default(),
            reference_list: Some(ReferenceListWrapper {
                references: vec![ReferenceEntry {
                    id: Some("27136076".into()),
                    source: Some("MED".into()),
                    title: Some("Wishbone paper.".into()),
                    author_string: Some("Setty M, Pe'er D.".into()),
                    journal_abbreviation: Some("Nat Biotechnol".into()),
                    pub_year: Some(2016),
                    volume: Some("34".into()),
                    page_info: Some("637-645".into()),
                    ..Default::default()
                }],
            }),
        };
        let rendered = format_references(&resp);
        assert!(rendered.contains("**19** references"));
        assert!(rendered.contains("Wishbone paper."));
        assert!(rendered.contains("Setty M"));
    }

    #[test]
    fn format_citations_basic() {
        let resp = CitationsResponse {
            version: "6.9".into(),
            hit_count: 5,
            request: ReferenceRequestEcho::default(),
            citation_list: Some(CitationListWrapper {
                citations: vec![CitationEntry {
                    id: Some("34921638".into()),
                    source: Some("MED".into()),
                    title: Some("Citing paper.".into()),
                    author_string: Some("Prodromidou K.".into()),
                    cited_by_count: Some(4),
                    ..Default::default()
                }],
            }),
        };
        let rendered = format_citations(&resp);
        assert!(rendered.contains("**5** citations"));
        assert!(rendered.contains("Citing paper."));
        assert!(rendered.contains("Cited by:** 4"));
    }
}
