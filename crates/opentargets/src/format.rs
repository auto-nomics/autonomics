//! Render Open Targets SDK responses into clean, LLM-friendly Markdown.
//!
//! Tabular results (search hits, associations) become Markdown tables;
//! single-entity lookups become structured key-value cards. All formatters
//! are pure functions over the typed SDK structs.

use crate::associations::{AssociatedDisease, AssociatedTarget};
use crate::search::{SearchResult, SearchResults};
use crate::types::{Disease, Drug, Study, Target, Variant};

/// Format a single line of comma-joined datasource scores, keeping only the
/// top contributors (score > 0) and capped to `max` entries.
fn top_datasources(scores: &[crate::types::ScoredComponent], max: usize) -> String {
    let mut sorted: Vec<_> = scores.iter().filter(|s| s.score > 0.0).collect();
    sorted.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
    sorted
        .iter()
        .take(max)
        .map(|s| format!("{}:{:.2}", s.id, s.score))
        .collect::<Vec<_>>()
        .join(", ")
}

// ---------------------------------------------------------------------------
// Search
// ---------------------------------------------------------------------------

/// Format search results as a compact Markdown table.
pub fn format_search(results: &SearchResults) -> String {
    if results.hits.is_empty() {
        return format!("No hits found (total reported: {}).", results.total);
    }
    let mut out = String::with_capacity(2048);
    out.push_str(&format!("**{} hits** (total {})\n\n", results.hits.len(), results.total));
    out.push_str("| # | Entity | ID | Name | Score | Description |\n");
    out.push_str("|---|---------|----|------|-------|-------------|\n");
    for (i, h) in results.hits.iter().enumerate() {
        out.push_str(&format!(
            "| {} | {} | `{}` | {} | {:.1} | {} |\n",
            i + 1,
            h.entity,
            h.id,
            h.name.replace('|', "\\|"),
            h.score,
            h.description.as_deref().unwrap_or("-").replace('|', "\\|"),
        ));
    }
    out
}

/// Render a single search hit (used by the lookup helpers).
pub fn format_search_hit(h: &SearchResult) -> String {
    format!(
        "| {} | `{}` | {} | {:.1} | {} |",
        h.entity,
        h.id,
        h.name,
        h.score,
        h.description.as_deref().unwrap_or("-")
    )
}

// ---------------------------------------------------------------------------
// Target
// ---------------------------------------------------------------------------

/// Format a target (gene) annotation as a Markdown card.
pub fn format_target(t: &Target) -> String {
    let mut out = String::with_capacity(1024);
    out.push_str(&format!("## {} ({})\n\n", t.approved_symbol, t.id));
    out.push_str(&format!("- **Approved name:** {}\n", t.approved_name));
    out.push_str(&format!("- **Biotype:** {}\n", t.biotype));
    out.push_str(&format!(
        "- **Location:** chr{}:{}-{} ({})\n",
        t.genomic_location.chromosome,
        t.genomic_location.start,
        t.genomic_location.end,
        strand_str(t.genomic_location.strand)
    ));
    if let Some(essential) = t.is_essential {
        out.push_str(&format!("- **Essential:** {}\n", essential));
    }
    if !t.function_descriptions.is_empty() {
        out.push_str("- **Function:**\n");
        for f in &t.function_descriptions {
            out.push_str(&format!("  - {f}\n"));
        }
    }
    out
}

fn strand_str(s: i32) -> &'static str {
    if s < 0 {
        "reverse"
    } else if s > 0 {
        "forward"
    } else {
        "unknown"
    }
}

// ---------------------------------------------------------------------------
// Disease
// ---------------------------------------------------------------------------

pub fn format_disease(d: &Disease) -> String {
    let mut out = String::with_capacity(512);
    out.push_str(&format!("## {} ({})\n\n", d.name, d.id));
    if let Some(ref desc) = d.description {
        if !desc.is_empty() {
            out.push_str(&format!("{desc}\n\n"));
        }
    }
    out.push_str(&format!("- **Therapeutic area:** {}\n", d.is_therapeutic_area));
    if !d.parents.is_empty() {
        let parents = disease_id_list(&d.parents);
        out.push_str(&format!("- **Parents:** {parents}\n"));
    }
    out
}

/// Extract `id` values from a JSON array of `{ id, name }` disease objects.
fn disease_id_list(arr: &[serde_json::Value]) -> String {
    arr.iter()
        .filter_map(|v| v.get("id").and_then(|i| i.as_str()).map(String::from))
        .collect::<Vec<_>>()
        .join(", ")
}

// ---------------------------------------------------------------------------
// Drug
// ---------------------------------------------------------------------------

pub fn format_drug(d: &Drug) -> String {
    let mut out = String::with_capacity(512);
    out.push_str(&format!("## {} ({})\n\n", d.name, d.id));
    out.push_str(&format!("- **Type:** {}\n", d.drug_type));
    out.push_str(&format!("- **Max clinical stage:** {}\n", d.maximum_clinical_stage));
    if let Some(ref desc) = d.description {
        if !desc.is_empty() {
            out.push_str(&format!("- **Description:** {desc}\n"));
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Study
// ---------------------------------------------------------------------------

pub fn format_study(s: &Study) -> String {
    let mut out = String::with_capacity(640);
    out.push_str(&format!("## {} ({})\n\n", s.trait_from_source, s.id));
    if let Some(ref t) = s.study_type {
        out.push_str(&format!("- **Study type:** {t}\n"));
    }
    out.push_str(&format!("- **Project:** {}\n", s.project_id));
    if let Some(ref p) = s.pubmed_id {
        if !p.is_empty() {
            out.push_str(&format!("- **PMID:** {p}\n"));
        }
    }
    if let Some(ref journal) = s.publication_journal {
        if !journal.is_empty() {
            out.push_str(&format!("- **Journal:** {journal}\n"));
        }
    }
    out.push_str(&format!(
        "- **Samples:** {} (cases: {}, controls: {})\n",
        s.n_samples.map(|n| n.to_string()).unwrap_or("-".into()),
        s.n_cases.map(|n| n.to_string()).unwrap_or("-".into()),
        s.n_controls.map(|n| n.to_string()).unwrap_or("-".into()),
    ));
    if let Some(has) = s.has_sumstats {
        out.push_str(&format!("- **Summary stats available:** {has}\n"));
    }
    out
}

// ---------------------------------------------------------------------------
// Variant
// ---------------------------------------------------------------------------

pub fn format_variant(v: &Variant) -> String {
    let mut out = String::with_capacity(512);
    out.push_str(&format!("## {} ({})\n\n", v.id, v.id));
    out.push_str(&format!(
        "- **Position:** chr{}:{} (GRCh38)\n",
        v.chromosome, v.position
    ));
    out.push_str(&format!(
        "- **Alleles:** {} > {}\n",
        v.reference_allele, v.alternate_allele
    ));
    if !v.rs_ids.is_empty() {
        out.push_str(&format!("- **rsIDs:** {}\n", v.rs_ids.join(", ")));
    }
    if !v.variant_description.is_empty() {
        out.push_str(&format!("- **Description:** {}\n", v.variant_description));
    }
    out
}

// ---------------------------------------------------------------------------
// Associations
// ---------------------------------------------------------------------------

/// Format a page of target→disease associations as a Markdown table.
///
/// `count` is the total number of associations available server-side.
pub fn format_associated_diseases(rows: &[AssociatedDisease], count: i64) -> String {
    let mut out = String::with_capacity(2048);
    out.push_str(&format!(
        "**{} of {} associated diseases** (sorted by overall score)\n\n",
        rows.len(),
        count
    ));
    if rows.is_empty() {
        return out;
    }
    out.push_str("| # | Score | Disease ID | Disease name | Novelty | Top datasources |\n");
    out.push_str("|---|-------|------------|--------------|---------|-----------------|\n");
    for (i, a) in rows.iter().enumerate() {
        out.push_str(&format!(
            "| {} | {:.3} | `{}` | {} | {} | {} |\n",
            i + 1,
            a.score,
            a.disease.id,
            a.disease.name.replace('|', "\\|"),
            a.novelty
                .map(|n| format!("{:.4}", n))
                .unwrap_or("-".into()),
            top_datasources(&a.datasource_scores, 5),
        ));
    }
    out
}

/// Format a page of disease→target associations as a Markdown table.
pub fn format_associated_targets(rows: &[AssociatedTarget], count: i64) -> String {
    let mut out = String::with_capacity(2048);
    out.push_str(&format!(
        "**{} of {} associated targets** (sorted by overall score)\n\n",
        rows.len(),
        count
    ));
    if rows.is_empty() {
        return out;
    }
    out.push_str("| # | Score | Target ID | Symbol | Name | Biotype | Top datasources |\n");
    out.push_str("|---|-------|-----------|--------|------|---------|-----------------|\n");
    for (i, a) in rows.iter().enumerate() {
        out.push_str(&format!(
            "| {} | {:.3} | `{}` | {} | {} | {} | {} |\n",
            i + 1,
            a.score,
            a.target.id,
            a.target.approved_symbol,
            a.target.approved_name.replace('|', "\\|"),
            a.target.biotype,
            top_datasources(&a.datasource_scores, 5),
        ));
    }
    out
}
