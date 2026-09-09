//! Markdown summaries for Reactome API responses, suitable for agent
//! tool previews and LLM consumption.

use crate::types::*;

fn option(value: Option<&str>) -> &str {
    value.unwrap_or("-")
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}…", &s[..s.char_indices().take(max).last().map_or(0, |(i, _)| i)])
    }
}

// ===========================================================================
// Database
// ===========================================================================

/// Format database info as a one-line summary.
pub fn format_database(info: &DatabaseInfo) -> String {
    format!("**{}** release {}\n", info.name, info.version)
}

// ===========================================================================
// Pathways
// ===========================================================================

/// Format a list of top-level pathways as a compact preview.
pub fn format_pathways(pathways: &[Pathway]) -> String {
    if pathways.is_empty() {
        return "No pathways found.\n".to_string();
    }
    let mut out = format!("**{}** pathways\n\n", pathways.len());
    for p in pathways.iter().take(30) {
        let id = p.stable_id.as_deref().unwrap_or("-");
        out.push_str(&format!(
            "- `{id}` {} ({})\n",
            p.display_name,
            option(p.schema_class.as_deref()),
        ));
    }
    if pathways.len() > 30 {
        out.push_str(&format!("\n_…and {} more_\n", pathways.len() - 30));
    }
    out
}

/// Format a single pathway in detail.
pub fn format_pathway(p: &Pathway) -> String {
    let mut out = format!(
        "## {}\n\n- ID: `{}`\n- Stable ID: `{}`\n- Class: {}\n",
        p.display_name,
        p.db_id,
        option(p.stable_id.as_deref()),
        option(p.schema_class.as_deref()),
    );
    if let Some(ref species) = p.species_name {
        out.push_str(&format!("- Species: {species}\n"));
    }
    if let Some(ref date) = p.release_date {
        out.push_str(&format!("- Release: {date}\n"));
    }
    out.push_str(&format!(
        "- Diagram: {}\n- EHLD: {}\n",
        if p.has_diagram { "yes" } else { "no" },
        if p.has_ehld { "yes" } else { "no" },
    ));
    out
}

// ===========================================================================
// Species
// ===========================================================================

/// Format a species list as a preview.
pub fn format_species(species: &[Species]) -> String {
    if species.is_empty() {
        return "No species found.\n".to_string();
    }
    let mut out = format!("**{}** species\n\n", species.len());
    for s in species.iter().take(20) {
        out.push_str(&format!(
            "- `{}` {} (taxId {})\n",
            s.display_name, s.abbreviation.as_deref().unwrap_or("-"), s.tax_id,
        ));
    }
    if species.len() > 20 {
        out.push_str(&format!("\n_…and {} more_\n", species.len() - 20));
    }
    out
}

// ===========================================================================
// Mapping
// ===========================================================================

/// Format mapped pathways from identifier lookup.
pub fn format_mapped_pathways(
    resource: &str,
    identifier: &str,
    pathways: &[MappedPathway],
) -> String {
    if pathways.is_empty() {
        return format!("No Reactome pathways found for `{identifier}` in *{resource}*.\n");
    }
    let mut out = format!(
        "**{}** pathways for `{identifier}` ({resource})\n\n",
        pathways.len()
    );
    for p in pathways.iter().take(20) {
        let id = p.stable_id.as_deref().unwrap_or("-");
        out.push_str(&format!("- `{id}` {}\n", p.display_name));
    }
    if pathways.len() > 20 {
        out.push_str(&format!("\n_…and {} more_\n", pathways.len() - 20));
    }
    out
}

// ===========================================================================
// Participants
// ===========================================================================

/// Format pathway or reaction participants.
pub fn format_participants(id: &str, participants: &[Participant]) -> String {
    if participants.is_empty() {
        return format!("No participants found for `{id}`.\n");
    }
    let mut out = format!("**{}** participants for `{id}`\n\n", participants.len());
    for p in participants.iter().take(20) {
        let name = p.display_name.as_deref().unwrap_or("-");
        let ref_id = p
            .reference_entity
            .as_ref()
            .and_then(|r| r.identifier.as_deref())
            .unwrap_or("-");
        let db = p
            .reference_entity
            .as_ref()
            .and_then(|r| r.database_name.as_deref())
            .unwrap_or("-");
        out.push_str(&format!("- {name} (`{ref_id}` via {db})\n"));
    }
    if participants.len() > 20 {
        out.push_str(&format!("\n_…and {} more_\n", participants.len() - 20));
    }
    out
}

// ===========================================================================
// Search
// ===========================================================================

/// Format full-text search results.
pub fn format_search(query: &str, result: &SearchResult) -> String {
    let n = result.results.len();
    if n == 0 {
        return format!("No results for `{query}`.\n");
    }
    let mut out = format!(
        "**{n}** results for `{query}` ({} matches total)\n\n",
        result.number_of_matches
    );
    for r in result.results.iter().take(20) {
        let id = r.stable_id.as_deref().unwrap_or("-");
        let t = r.entry_type.as_deref().unwrap_or("-");
        let name = r.display_name.as_deref().unwrap_or("-");
        out.push_str(&format!("- `{id}` [{t}] {name}\n"));
    }
    if n > 20 {
        out.push_str(&format!("\n_…and {} more_\n", n - 20));
    }
    out
}

// ===========================================================================
// Analysis
// ===========================================================================

/// Format an over-representation analysis result as a rich Markdown summary.
pub fn format_analysis(result: &AnalysisResult) -> String {
    let s = &result.summary;
    let mapped = result.pathways_found + result.identifiers_not_found;
    let mut out = format!(
        "## Pathway Analysis\n\n\
         - Token: `{}`\n\
         - Type: {}\n\
         - Identifiers mapped: {}\n\
         - Identifiers not found: {}\n\
         - Pathways hit: {}\n\n",
        s.token,
        s.analysis_type.as_deref().unwrap_or("-"),
        mapped,
        result.identifiers_not_found,
        result.pathways_found,
    );

    if result.pathways.is_empty() {
        out.push_str("No significantly enriched pathways.\n");
        return out;
    }

    out.push_str("| Pathway | Stable ID | Found/Total | p-value | FDR |\n");
    out.push_str("|---------|-----------|-------------|---------|-----|\n");
    for p in result.pathways.iter().take(15) {
        let e = &p.entities;
        let pv = e
            .p_value
            .map(|v| format!("{v:.2e}"))
            .unwrap_or_else(|| "-".to_string());
        let fdr = e
            .fdr
            .map(|v| format!("{v:.2e}"))
            .unwrap_or_else(|| "-".to_string());
        out.push_str(&format!(
            "| {} | `{}` | {}/{} | {} | {} |\n",
            truncate(&p.name, 60),
            p.stable_id,
            e.found,
            e.total,
            pv,
            fdr,
        ));
    }
    if result.pathways.len() > 15 {
        out.push_str(&format!(
            "\n_…and {} more pathways_\n",
            result.pathways.len() - 15
        ));
    }

    if result.identifiers_not_found > 0 {
        out.push_str(&format!(
            "\n**Identifiers not found:** {}\n",
            result.identifiers_not_found
        ));
    }
    out
}
