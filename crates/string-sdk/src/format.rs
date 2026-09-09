//! Compact Markdown summaries for preview-oriented callers.

use crate::types::*;

fn truncate(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        text.to_owned()
    } else {
        let cut = text
            .char_indices()
            .nth(limit)
            .map_or(text.len(), |(i, _)| i);
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

/// Summarize identifier mappings as a Markdown table.
pub fn format_string_ids(rows: &[StringId], limit: usize) -> String {
    let mut out = format!(
        "## STRING identifier mappings\n\nShowing {} of {}.\n\n",
        rows.len().min(limit),
        rows.len()
    );
    if rows.is_empty() {
        out.push_str("No identifiers were resolved.\n");
        return out.trim_end().to_owned();
    }
    out.push_str("| Input | STRING ID | Preferred name | Taxon |\n|---|---|---|---|\n");
    for row in rows.iter().take(limit) {
        out.push_str(&format!(
            "| `{}` | `{}` | {} | {} |\n",
            optional(row.query_item.as_deref()),
            row.string_id,
            optional(row.preferred_name.as_deref()),
            optional(row.taxon_name.as_deref()),
        ));
    }
    out.trim_end().to_owned()
}

/// Summarize interactions with combined and experimental scores.
pub fn format_interactions(rows: &[Interaction], limit: usize) -> String {
    let mut out = format!(
        "## STRING interactions\n\nShowing {} of {}.\n\n",
        rows.len().min(limit),
        rows.len()
    );
    if rows.is_empty() {
        out.push_str("No interactions were returned.\n");
        return out.trim_end().to_owned();
    }
    out.push_str("| Protein A | Protein B | Combined | Experimental |\n|---|---|---:|---:|\n");
    for row in rows.iter().take(limit) {
        out.push_str(&format!(
            "| `{}` | `{}` | {:.3} | {:.3} |\n",
            optional(
                row.preferred_name_a
                    .as_deref()
                    .or(Some(row.string_id_a.as_str()))
            ),
            optional(
                row.preferred_name_b
                    .as_deref()
                    .or(Some(row.string_id_b.as_str()))
            ),
            row.score,
            row.escore,
        ));
    }
    out.trim_end().to_owned()
}

/// Summarize enrichment rows, retaining the most significant terms first.
pub fn format_enrichment(rows: &[Enrichment], limit: usize) -> String {
    let mut sorted: Vec<&Enrichment> = rows.iter().collect();
    sorted.sort_by(|a, b| {
        a.fdr
            .total_cmp(&b.fdr)
            .then(a.p_value.total_cmp(&b.p_value))
    });
    let mut out = format!(
        "## STRING functional enrichment\n\nShowing {} of {} terms.\n\n",
        sorted.len().min(limit),
        sorted.len()
    );
    if sorted.is_empty() {
        out.push_str("No enriched terms were returned.\n");
        return out.trim_end().to_owned();
    }
    out.push_str("| Category | Term | FDR | Genes | Description |\n|---|---|---:|---:|---|\n");
    for row in sorted.into_iter().take(limit) {
        out.push_str(&format!(
            "| {} | `{}` | {} | {}/{} | {} |\n",
            row.category,
            row.term,
            compact_number(row.fdr),
            row.number_of_genes,
            row.number_of_genes_in_background,
            optional(row.description.as_deref()),
        ));
    }
    out.trim_end().to_owned()
}

/// Summarize network-level interaction enrichment.
pub fn format_ppi_enrichment(rows: &[PpiEnrichment]) -> String {
    let mut out = String::from("## STRING PPI enrichment\n\n");
    if rows.is_empty() {
        out.push_str("No result was returned.\n");
        return out.trim_end().to_owned();
    }
    out.push_str("| Nodes | Edges | Expected edges | Average degree | P-value |\n|---:|---:|---:|---:|---:|\n");
    for row in rows {
        out.push_str(&format!(
            "| {} | {} | {:.3} | {:.3} | {} |\n",
            row.number_of_nodes,
            row.number_of_edges,
            row.expected_number_of_edges,
            row.average_node_degree,
            compact_number(row.p_value),
        ));
    }
    out.trim_end().to_owned()
}

/// Summarize a Values/Ranks job without exposing internal task state.
pub fn format_valuesranks_job(job: &ValuesRanksJob) -> String {
    let mut out = format!(
        "## Values/Ranks job\n\n- Job ID: `{}`\n- Status: {}\n",
        job.job_id, job.status
    );
    if let Some(message) = job.message.as_deref() {
        out.push_str(&format!("- Message: {}\n", truncate(message, 500)));
    }
    if let Some(url) = job.page_url.as_deref() {
        out.push_str(&format!("- Result page: {url}\n"));
    }
    if let Some(url) = job.download_url.as_deref() {
        out.push_str(&format!("- Download: {url}\n"));
    }
    if let Some(url) = job.graph_url.as_deref() {
        out.push_str(&format!("- Figure: {url}\n"));
    }
    out.trim_end().to_owned()
}
