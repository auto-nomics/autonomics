//! Compact Markdown summaries for preview-oriented callers.

use crate::types::*;

fn truncate(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        text.to_owned()
    } else {
        let cut = text
            .char_indices()
            .nth(limit)
            .map_or(text.len(), |(index, _)| index);
        format!("{}...", &text[..cut])
    }
}

fn optional(value: Option<&str>) -> &str {
    value.unwrap_or("-")
}

fn compact_number(value: f64) -> String {
    if value == 0.0 || !(0.0001..=9999.5).contains(&value.abs()) {
        format!("{value:.3e}")
    } else {
        format!("{value:.3}")
    }
}

/// Summarize available gene-set libraries as a Markdown table.
pub fn format_libraries(rows: &[&LibraryStats], total: usize, limit: usize) -> String {
    let mut out = format!(
        "## Enrichr libraries\n\nShowing {} of {total} matching libraries.\n\n",
        rows.len().min(limit),
    );
    if rows.is_empty() {
        out.push_str("No libraries matched. Omit the query to list every library.\n");
        return out.trim_end().to_owned();
    }
    out.push_str("| Library | Terms | Genes/term | Coverage |\n|---|---:|---:|---:|\n");
    for row in rows.iter().take(limit) {
        let per_term = row.genes_per_term.map(|value| format!("{value:.1}"));
        out.push_str(&format!(
            "| `{}` | {} | {} | {} |\n",
            row.library_name,
            row.num_terms
                .map(|n| n.to_string())
                .unwrap_or_else(|| "-".into()),
            per_term.as_deref().unwrap_or("-"),
            row.gene_coverage
                .map(|n| n.to_string())
                .unwrap_or_else(|| "-".into()),
        ));
    }
    out.trim_end().to_owned()
}

/// Summarize enrichment terms, most significant first.
pub fn format_enrichment(result: &EnrichmentResult, limit: usize) -> String {
    let mut out = format!(
        "## Enrichr enrichment — {}\n\nShowing {} of {} terms.\n\n",
        result.library,
        result.terms.len().min(limit),
        result.terms.len(),
    );
    if result.terms.is_empty() {
        out.push_str("No enriched terms were returned.\n");
        return out.trim_end().to_owned();
    }
    out.push_str(
        "| Rank | Term | Adj. p | P-value | Score | Combined | Genes |\n\
         |---:|---|---:|---:|---:|---:|---|\n",
    );
    for term in result.terms.iter().take(limit) {
        out.push_str(&format!(
            "| {} | {} | {} | {} | {} | {} | {} ({}) |\n",
            term.rank,
            truncate(&term.term, 60),
            compact_number(term.adjusted_p_value),
            compact_number(term.p_value),
            compact_number(term.z_score),
            compact_number(term.combined_score),
            truncate(&term.overlapping_genes.join(","), 40),
            term.overlap_count(),
        ));
    }
    out.trim_end().to_owned()
}

/// Summarize a submitted gene list.
pub fn format_viewed_list(viewed: &ViewedList) -> String {
    let mut out = format!(
        "## Enrichr gene list\n\n- Genes: {}\n- Description: {}\n",
        viewed.genes.len(),
        optional(viewed.description.as_deref()),
    );
    out.push_str(&format!("\n{}\n", truncate(&viewed.genes.join(", "), 600)));
    out.trim_end().to_owned()
}

/// Summarize an `addList` acknowledgement with its share link.
pub fn format_added_list(added: &AddedList, share_url: &str) -> String {
    let mut out = format!(
        "## Enrichr list submitted\n\n- User list ID: `{}`\n",
        added.user_list_id,
    );
    if let Some(short_id) = added.short_id.as_deref().filter(|id| !id.is_empty()) {
        out.push_str(&format!("- Short ID: `{short_id}`\n- Share: {share_url}\n"));
    }
    out.push_str("\nReuse the user list ID for enrich, view, and export calls.\n");
    out.trim_end().to_owned()
}

/// Summarize a genemap lookup.
pub fn format_gene_map(map: &GeneMap, limit: usize) -> String {
    let mut out = format!(
        "## Enrichr gene map — {}\n\n{} terms across {} libraries.\n\n",
        map.gene,
        map.term_count(),
        map.libraries.len(),
    );
    if map.libraries.is_empty() {
        out.push_str("No annotated terms were found for this gene.\n");
        return out.trim_end().to_owned();
    }
    out.push_str("| Library | Terms |\n|---|---:|\n");
    for (library, terms) in map.libraries.iter().take(limit) {
        out.push_str(&format!(
            "| `{}` | {} (e.g. {}) |\n",
            library,
            terms.len(),
            truncate(terms.first().map(String::as_str).unwrap_or("-"), 50),
        ));
    }
    out.trim_end().to_owned()
}
