//! Render Semantic Scholar API responses into clean, LLM-friendly Markdown.

use crate::types::*;

// ===========================================================================
// Search results
// ===========================================================================

/// Format a paper search response into readable Markdown.
pub fn format_search(resp: &PaperSearchResponse) -> String {
    let mut out = String::with_capacity(8192);

    out.push_str(&format!("**{}** results found", resp.total));
    let n = resp.data.len();
    if n > 0 {
        out.push_str(&format!(" (showing {})\n\n", n));
    } else {
        out.push_str("\n\nNo entries returned.\n");
        return out;
    }

    for (i, paper) in resp.data.iter().enumerate() {
        format_paper_preview(i + 1, paper, &mut out);
        out.push('\n');
    }

    if let Some(next) = resp.next {
        out.push_str(&format!("_Next page offset:_ `{next}`\n\n"));
    }

    out.trim_end().to_string()
}

// ---------------------------------------------------------------------------
// Paper detail
// ---------------------------------------------------------------------------

/// Format a single paper with full detail.
pub fn format_paper(paper: &Paper) -> String {
    let mut out = String::with_capacity(8192);
    format_paper_full(paper, &mut out);
    out.trim_end().to_string()
}

// ---------------------------------------------------------------------------
// Citations
// ---------------------------------------------------------------------------

/// Format a citations response into readable Markdown.
pub fn format_citations(resp: &CitationBatchResponse) -> String {
    let mut out = String::with_capacity(8192);
    out.push_str(&format!("Showing {} citations", resp.data.len()));
    if let Some(next) = resp.next {
        out.push_str(&format!(" (more at offset {next})"));
    }
    out.push_str("\n\n");

    if resp.data.is_empty() {
        out.push_str("No citations found.\n");
        return out;
    }

    for (i, c) in resp.data.iter().enumerate() {
        format_paper_preview(i + 1, &c.citing_paper, &mut out);
        if let Some(ref intents) = c.intents {
            if !intents.is_empty() {
                out.push_str(&format!("**Intents:** {}\n", intents.join(", ")));
            }
        }
        if let Some(inf) = c.is_influential {
            if inf {
                out.push_str("**Influential:** yes\n");
            }
        }
        out.push('\n');
    }

    out.trim_end().to_string()
}

// ---------------------------------------------------------------------------
// References
// ---------------------------------------------------------------------------

/// Format a references response into readable Markdown.
pub fn format_references(resp: &ReferenceBatchResponse) -> String {
    let mut out = String::with_capacity(8192);
    out.push_str(&format!("Showing {} references", resp.data.len()));
    if let Some(next) = resp.next {
        out.push_str(&format!(" (more at offset {next})"));
    }
    out.push_str("\n\n");

    if resp.data.is_empty() {
        out.push_str("No references found.\n");
        return out;
    }

    for (i, r) in resp.data.iter().enumerate() {
        format_paper_preview(i + 1, &r.cited_paper, &mut out);
        if let Some(ref intents) = r.intents {
            if !intents.is_empty() {
                out.push_str(&format!("**Intents:** {}\n", intents.join(", ")));
            }
        }
        if let Some(inf) = r.is_influential {
            if inf {
                out.push_str("**Influential:** yes\n");
            }
        }
        out.push('\n');
    }

    out.trim_end().to_string()
}

// ---------------------------------------------------------------------------
// Authors
// ---------------------------------------------------------------------------

/// Format an author search response into readable Markdown.
pub fn format_author_search(resp: &AuthorSearchResponse) -> String {
    let mut out = String::with_capacity(4096);
    out.push_str(&format!("**{}** authors found", resp.total));
    let n = resp.data.len();
    if n > 0 {
        out.push_str(&format!(" (showing {})\n\n", n));
    } else {
        out.push_str("\n\nNo authors found.\n");
        return out;
    }

    for (i, a) in resp.data.iter().enumerate() {
        format_author(i + 1, a, &mut out);
        out.push('\n');
    }

    out.trim_end().to_string()
}

/// Format a single author's details.
pub fn format_author_detail(a: &Author) -> String {
    let mut out = String::with_capacity(4096);
    format_author(0, a, &mut out);
    out.trim_end().to_string()
}

// ---------------------------------------------------------------------------
// Recommendations
// ---------------------------------------------------------------------------

/// Format a recommendations response into readable Markdown.
pub fn format_recommendations(resp: &RecommendationsResponse) -> String {
    let mut out = String::with_capacity(8192);
    let n = resp.recommended_papers.len();
    out.push_str(&format!("**{}** recommended papers\n\n", n));

    for (i, p) in resp.recommended_papers.iter().enumerate() {
        format_paper_preview(i + 1, p, &mut out);
        out.push('\n');
    }

    out.trim_end().to_string()
}

// ===========================================================================
// Per-entry formatters
// ===========================================================================

pub(crate) fn format_paper_preview(i: usize, paper: &Paper, out: &mut String) {
    out.push_str(&format!(
        "### {}. {}\n",
        i,
        paper.title.as_deref().unwrap_or("(untitled)")
    ));

    // paperId
    out.push_str(&format!("**Paper ID:** {}\n", paper.paper_id));

    // Authors
    if !paper.authors.is_empty() {
        let display = if paper.authors.len() > 5 {
            let first = paper.authors.first().and_then(|a| a.name.as_deref()).unwrap_or("");
            format!("{} et al.", first)
        } else {
            paper
                .authors
                .iter()
                .filter_map(|a| a.name.as_deref())
                .collect::<Vec<_>>()
                .join(", ")
        };
        out.push_str(&format!("**Authors:** {display}\n"));
    }

    // Year
    if let Some(y) = paper.year {
        out.push_str(&format!("**Year:** {y}\n"));
    }

    // Venue
    if let Some(ref v) = paper.venue {
        if !v.is_empty() {
            out.push_str(&format!("**Venue:** {v}\n"));
        }
    }

    // DOI
    if let Some(ref ext) = paper.external_ids {
        if let Some(ref doi) = ext.doi {
            out.push_str(&format!("**DOI:** {doi}\n"));
        }
    }

    // Citation count
    if let Some(c) = paper.citation_count {
        if c > 0 {
            out.push_str(&format!("**Citations:** {c}\n"));
        }
    }

    // TLDR
    if let Some(ref tldr) = paper.tldr {
        if let Some(ref text) = tldr.text {
            if !text.is_empty() {
                let preview = truncate_at(text, 300);
                out.push_str(&format!("\n> **TL;DR:** {preview}\n"));
                if text.len() > 300 {
                    out.push_str(">\n> ...\n");
                }
            }
        }
    }

    // Abstract (truncated for preview)
    if let Some(ref abs) = paper.abstract_text {
        if !abs.is_empty() {
            let preview = truncate_at(abs, 400);
            out.push_str(&format!("\n> {preview}\n"));
            if abs.len() > 400 {
                out.push_str("> ...\n");
            }
        }
    }

    out.push('\n');
}

fn format_paper_full(paper: &Paper, out: &mut String) {
    out.push_str(&format!(
        "## {}\n\n",
        paper.title.as_deref().unwrap_or("(untitled)")
    ));

    out.push_str(&format!("**Paper ID:** {}\n", paper.paper_id));

    if let Some(c) = paper.corpus_id {
        out.push_str(&format!("**Corpus ID:** {c}\n"));
    }

    if let Some(ref url) = paper.url {
        out.push_str(&format!("**URL:** {url}\n"));
    }

    // Full author list
    if !paper.authors.is_empty() {
        let parts: Vec<String> = paper
            .authors
            .iter()
            .map(|a| {
                let name = a.name.as_deref().unwrap_or("Unknown");
                match &a.author_id {
                    Some(id) if !id.is_empty() => format!("{name} [{id}]"),
                    _ => name.to_string(),
                }
            })
            .collect();
        out.push_str(&format!("**Authors:** {}\n", parts.join(", ")));
    }

    if let Some(y) = paper.year {
        out.push_str(&format!("**Year:** {y}\n"));
    }

    if let Some(ref v) = paper.venue {
        if !v.is_empty() {
            out.push_str(&format!("**Venue:** {v}\n"));
        }
    }

    // External IDs
    if let Some(ref ext) = paper.external_ids {
        if let Some(ref doi) = ext.doi {
            out.push_str(&format!("**DOI:** {doi}\n"));
        }
        if let Some(ref arxiv) = ext.arxiv {
            out.push_str(&format!("**ArXiv:** {arxiv}\n"));
        }
        if let Some(ref pmid) = ext.pubmed {
            out.push_str(&format!("**PMID:** {pmid}\n"));
        }
        if let Some(ref pmc) = ext.pubmed_central {
            out.push_str(&format!("**PMCID:** {pmc}\n"));
        }
    }

    // Publication date
    if let Some(ref pd) = paper.publication_date {
        out.push_str(&format!("**Published:** {pd}\n"));
    }

    // Publication types
    if let Some(ref pts) = paper.publication_types {
        if !pts.is_empty() {
            out.push_str(&format!("**Pub types:** {}\n", pts.join(", ")));
        }
    }

    // Counts
    if let Some(c) = paper.citation_count {
        out.push_str(&format!("**Citations:** {c}\n"));
    }
    if let Some(r) = paper.reference_count {
        out.push_str(&format!("**References:** {r}\n"));
    }
    if let Some(ic) = paper.influential_citation_count {
        if ic > 0 {
            out.push_str(&format!("**Influential citations:** {ic}\n"));
        }
    }

    // Open access
    if let Some(oa) = paper.is_open_access {
        out.push_str(&format!("**Open access:** {}\n", if oa { "yes" } else { "no" }));
    }
    if let Some(ref pdf) = paper.open_access_pdf {
        if let Some(ref url) = pdf.url {
            out.push_str(&format!("**PDF:** {url}\n"));
        }
    }

    // Fields of study
    if let Some(ref fos) = paper.fields_of_study {
        if !fos.is_empty() {
            out.push_str(&format!("**Fields:** {}\n", fos.join(", ")));
        }
    }

    // Journal
    if let Some(ref j) = paper.journal {
        if let Some(ref name) = j.name {
            out.push_str(&format!("**Journal:** {name}\n"));
        }
        if let Some(ref vol) = j.volume {
            out.push_str(&format!("**Volume:** {vol}\n"));
        }
        if let Some(ref pages) = j.pages {
            out.push_str(&format!("**Pages:** {pages}\n"));
        }
    }

    // TLDR
    if let Some(ref tldr) = paper.tldr {
        if let Some(ref text) = tldr.text {
            out.push_str(&format!("\n**TL;DR:** {text}\n"));
        }
    }

    // Full abstract
    if let Some(ref abs) = paper.abstract_text {
        if !abs.is_empty() {
            out.push_str("\n**Abstract:**\n\n");
            out.push_str(abs);
            out.push('\n');
        }
    }

    out.push('\n');
}

fn format_author(i: usize, a: &Author, out: &mut String) {
    if i > 0 {
        out.push_str(&format!("### {}. {}\n", i, a.name.as_deref().unwrap_or("(unknown)")));
    } else {
        out.push_str(&format!("## {}\n\n", a.name.as_deref().unwrap_or("(unknown)")));
    }

    out.push_str(&format!("**Author ID:** {}\n", a.author_id));

    if let Some(ref url) = a.url {
        out.push_str(&format!("**URL:** {url}\n"));
    }

    if let Some(ref affs) = a.affiliations {
        if !affs.is_empty() {
            out.push_str(&format!("**Affiliations:** {}\n", affs.join(", ")));
        }
    }

    if let Some(ref hp) = a.homepage {
        if !hp.is_empty() {
            out.push_str(&format!("**Homepage:** {hp}\n"));
        }
    }

    if let Some(pc) = a.paper_count {
        out.push_str(&format!("**Papers:** {pc}\n"));
    }
    if let Some(cc) = a.citation_count {
        out.push_str(&format!("**Citations:** {cc}\n"));
    }
    if let Some(h) = a.h_index {
        out.push_str(&format!("**h-index:** {h}\n"));
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

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_paper() -> Paper {
        Paper {
            paper_id: "abc123".into(),
            title: Some("Deep learning for genomics".into()),
            year: Some(2024),
            venue: Some("Nature Biotechnology".into()),
            authors: vec![
                PaperAuthor { author_id: Some("1".into()), name: Some("Alice Smith".into()) },
                PaperAuthor { author_id: Some("2".into()), name: Some("Bob Jones".into()) },
            ],
            citation_count: Some(42),
            external_ids: Some(ExternalIds {
                doi: Some("10.1038/nbt.1234".into()),
                ..Default::default()
            }),
            tldr: Some(Tldr {
                model: Some("tldr@v2".into()),
                text: Some("A groundbreaking study on DL in genomics.".into()),
            }),
            ..Default::default()
        }
    }

    #[test]
    fn format_search_basic() {
        let resp = PaperSearchResponse {
            total: 100,
            offset: 0,
            next: Some(10),
            data: vec![sample_paper()],
        };
        let rendered = format_search(&resp);
        assert!(rendered.contains("**100** results"));
        assert!(rendered.contains("Deep learning for genomics"));
        assert!(rendered.contains("Paper ID:** abc123"));
        assert!(rendered.contains("Alice Smith"));
        assert!(rendered.contains("TL;DR"));
    }

    #[test]
    fn format_paper_detail() {
        let rendered = format_paper(&sample_paper());
        assert!(rendered.contains("## Deep learning for genomics"));
        assert!(rendered.contains("DOI:** 10.1038"));
        assert!(rendered.contains("Citations:** 42"));
    }

    #[test]
    fn format_citations_basic() {
        let resp = CitationBatchResponse {
            offset: 0,
            next: Some(10),
            data: vec![CitationEntry {
                intents: Some(vec!["methodology".into()]),
                is_influential: Some(true),
                citing_paper: sample_paper(),
                ..Default::default()
            }],
        };
        let rendered = format_citations(&resp);
        assert!(rendered.contains("Showing 1 citations"));
        assert!(rendered.contains("Intents:** methodology"));
        assert!(rendered.contains("Influential:** yes"));
    }
}
