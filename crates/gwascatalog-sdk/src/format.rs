//! Render SDK responses into clean, LLM-friendly Markdown.

use crate::rest::{
    EfoTrait, EmbeddedEfoTraits, EmbeddedRestAssociations, EmbeddedRestStudies, EmbeddedSnps,
    EmbeddedUnpublishedStudies, RestAssociation, RestStudy, Snp, UnpublishedStudy,
};
use crate::search::SearchResponse;
use crate::summary_stats::{Association, EmbeddedAssociations, PaginatedResponse};

// ── Solr Search ─────────────────────────────────────────────────────────────

pub fn format_search(resp: &SearchResponse) -> String {
    let docs = &resp.response.docs;
    if docs.is_empty() {
        return format!(
            "**No results found.** ({} in index)\n",
            resp.response.num_found
        );
    }

    let mut out = String::with_capacity(4096);
    out.push_str(&format!(
        "**{} results** (showing {})\n\n",
        resp.response.num_found,
        docs.len()
    ));

    out.push_str("| # | Type | Label | Key ID |\n");
    out.push_str("|---|------|-------|--------|\n");
    for (i, doc) in docs.iter().enumerate() {
        let rtype = doc.resourcename.as_deref().unwrap_or("-");
        let label = doc.label();
        let id = doc
            .accession_id
            .as_deref()
            .or(doc.rs_id.as_deref())
            .or(doc.short_form.as_deref())
            .or(doc.ensembl_id.as_deref())
            .or(doc.pmid.as_deref())
            .unwrap_or("-");
        out.push_str(&format!("| {} | {} | {} | {} |\n", i + 1, rtype, label, id));
    }

    // Facet summary
    if let Some(fc) = &resp.facet_counts {
        if let Some(rn_facets) = fc.facet_fields.get("resourcename") {
            out.push_str("\n**Resource types:** ");
            let parts: Vec<String> = rn_facets
                .chunks(2)
                .filter_map(|chunk| {
                    let name = chunk.first()?.as_str()?;
                    let count = chunk.get(1)?.as_u64()?;
                    Some(format!("{name} ({count})"))
                })
                .collect();
            out.push_str(&parts.join(", "));
            out.push('\n');
        }
    }

    out
}

// ── REST Studies ────────────────────────────────────────────────────────────

pub fn format_rest_studies(embedded: &EmbeddedRestStudies) -> String {
    if embedded.studies.is_empty() {
        return "No studies found.\n".to_string();
    }
    let mut out = String::with_capacity(4096);
    out.push_str(&format!("**{} studies**\n\n", embedded.studies.len()));

    for s in &embedded.studies {
        out.push_str(&format!(
            "### {} {}\n",
            s.accession_id,
            s.disease_trait
                .as_ref()
                .map(|t| t.trait_name.as_str())
                .unwrap_or("")
        ));
        if let Some(pi) = &s.publication_info {
            if let Some(title) = &pi.title {
                out.push_str(&format!("**Title:** {title}\n"));
            }
            if let Some(pmid) = &pi.pubmed_id {
                out.push_str(&format!("**PMID:** {pmid}\n"));
            }
            if let Some(date) = &pi.publication_date {
                out.push_str(&format!("**Date:** {date}\n"));
            }
            if let Some(author) = &pi.author {
                if let Some(name) = &author.fullname {
                    out.push_str(&format!("**Author:** {name}\n"));
                }
            }
        }
        if !s.initial_sample_size.is_empty() {
            out.push_str(&format!("**Sample:** {}\n", s.initial_sample_size));
        }
        if s.full_pvalue_set {
            out.push_str("**Full p-value set:** yes\n");
        }
        out.push('\n');
    }
    out
}

// ── REST Associations ───────────────────────────────────────────────────────

pub fn format_rest_associations(embedded: &EmbeddedRestAssociations) -> String {
    if embedded.associations.is_empty() {
        return "No associations found.\n".to_string();
    }
    let mut out = String::with_capacity(4096);
    out.push_str(&format!(
        "**{} associations**\n\n",
        embedded.associations.len()
    ));

    out.push_str("| rsID | p-value | OR / Beta | Genes |\n");
    out.push_str("|------|---------|-----------|-------|\n");
    for a in &embedded.associations {
        let rs = a.rsid().unwrap_or("-");
        let pval = format_pvalue(a.pvalue, a.pvalue_mantissa, a.pvalue_exponent);
        let effect = format_effect(a);
        let genes = a.genes().join(", ");
        out.push_str(&format!("| {} | {} | {} | {} |\n", rs, pval, effect, genes));
    }
    out
}

fn format_pvalue(pval: Option<f64>, mantissa: Option<u32>, exponent: Option<i32>) -> String {
    if let Some(v) = pval {
        return format!("{v:.2e}");
    }
    match (mantissa, exponent) {
        (Some(m), Some(e)) => format!("{m}×10^{e}"),
        _ => "-".to_string(),
    }
}

fn format_effect(a: &RestAssociation) -> String {
    if let Some(or) = a.or_per_copy_num {
        let ci = a
            .standard_error
            .map(|se| format!(" ± {se:.3}"))
            .unwrap_or_default();
        return format!("OR {or:.3}{ci}");
    }
    if let Some(beta) = a.beta_num {
        let dir = a
            .beta_direction
            .as_deref()
            .unwrap_or("")
            .chars()
            .next()
            .map(|c| format!(" ({c})"))
            .unwrap_or_default();
        return format!("β {beta:.3}{dir}");
    }
    "-".to_string()
}

// ── REST EFO Traits ─────────────────────────────────────────────────────────

pub fn format_rest_efo_traits(embedded: &EmbeddedEfoTraits) -> String {
    if embedded.efo_traits.is_empty() {
        return "No EFO traits found.\n".to_string();
    }
    let mut out = String::with_capacity(2048);
    out.push_str(&format!("**{} EFO traits**\n\n", embedded.efo_traits.len()));
    out.push_str("| Trait | Short Form | URI |\n");
    out.push_str("|-------|------------|-----|\n");
    for t in &embedded.efo_traits {
        out.push_str(&format!(
            "| {} | {} | {} |\n",
            t.trait_name, t.short_form, t.uri
        ));
    }
    out
}

// ── REST SNPs ───────────────────────────────────────────────────────────────

pub fn format_rest_snps(embedded: &EmbeddedSnps) -> String {
    if embedded.single_nucleotide_polymorphisms.is_empty() {
        return "No SNPs found.\n".to_string();
    }
    let mut out = String::with_capacity(4096);
    out.push_str(&format!(
        "**{} SNPs**\n\n",
        embedded.single_nucleotide_polymorphisms.len()
    ));
    for s in &embedded.single_nucleotide_polymorphisms {
        let loc = s.primary_location();
        let chrom = loc.map(|l| l.chromosome_name.as_str()).unwrap_or("-");
        let pos = loc
            .map(|l| l.chromosome_position.to_string())
            .unwrap_or_default();
        let region = loc
            .and_then(|l| l.region.as_ref())
            .and_then(|r| r.name.as_deref())
            .unwrap_or("-");
        out.push_str(&format!(
            "### {} (chr{}:{} {})\n",
            s.rs_id, chrom, pos, region
        ));
        if let Some(fc) = &s.functional_class {
            out.push_str(&format!("**Functional class:** {fc}\n"));
        }
        let genes = s.nearby_genes();
        if !genes.is_empty() {
            out.push_str(&format!("**Nearby genes:** {}\n", genes.join(", ")));
        }
        out.push('\n');
    }
    out
}

// ── Unpublished Studies ─────────────────────────────────────────────────────

pub fn format_unpublished(embedded: &EmbeddedUnpublishedStudies) -> String {
    if embedded.unpublished_studies.is_empty() {
        return "No unpublished studies found.\n".to_string();
    }
    let mut out = String::with_capacity(4096);
    out.push_str(&format!(
        "**{} unpublished studies**\n\n",
        embedded.unpublished_studies.len()
    ));
    out.push_str("| Accession | Trait | Description |\n");
    out.push_str("|-----------|-------|-------------|\n");
    for s in &embedded.unpublished_studies {
        let desc = if s.study_description.len() > 80 {
            format!("{}…", &s.study_description[..80])
        } else {
            s.study_description.clone()
        };
        out.push_str(&format!(
            "| {} | {} | {} |\n",
            s.study_accession, s.trait_name, desc
        ));
    }
    out
}

// ── Summary Statistics Associations ─────────────────────────────────────────

pub fn format_summary_associations(resp: &PaginatedResponse<EmbeddedAssociations>) -> String {
    let empty_embedded: EmbeddedAssociations = EmbeddedAssociations::default();
    let embedded = resp._embedded.as_ref().unwrap_or(&empty_embedded);
    if embedded.associations.is_empty() {
        return "No summary-statistics associations found.\n".to_string();
    }

    let mut out = String::with_capacity(4096);
    out.push_str(&format!(
        "**{} associations**\n\n",
        embedded.associations.len()
    ));
    out.push_str("| Variant | Chr | BP | p-value | Beta (SE) | OR (CI) | EA / OA | EAF |\n");
    out.push_str("|---------|-----|----|---------|-----------|---------|---------|-----|\n");
    for a in embedded.associations.values() {
        out.push_str(&format!(
            "| {} | {} | {} | {} | {} | {} | {} | {} |\n",
            a.variant_id,
            a.chromosome,
            a.base_pair_location,
            format!("{:.2e}", a.p_value),
            format_beta(a),
            format_or(a),
            format_alleles(a),
            a.effect_allele_frequency
                .map(|f| format!("{f:.3}"))
                .unwrap_or_else(|| "-".to_string()),
        ));
    }
    out
}

fn format_beta(a: &Association) -> String {
    match (a.beta, a.se) {
        (Some(b), Some(se)) => format!("{b:.4} ({se:.4})"),
        (Some(b), None) => format!("{b:.4}"),
        _ => "-".to_string(),
    }
}

fn format_or(a: &Association) -> String {
    match (a.odds_ratio, a.ci_lower, a.ci_upper) {
        (Some(or), Some(lo), Some(hi)) => format!("{or:.3} [{lo:.3}, {hi:.3}]"),
        (Some(or), _, _) => format!("{or:.3}"),
        _ => "-".to_string(),
    }
}

fn format_alleles(a: &Association) -> String {
    let ea = a.effect_allele.as_deref().unwrap_or("?");
    let oa = a.other_allele.as_deref().unwrap_or("?");
    format!("{ea} / {oa}")
}

// ── Download ───────────────────────────────────────────────────────────────

/// Format download results (structured as `{ accession, count, files, errors? }`).
pub fn format_download(value: &serde_json::Value) -> String {
    let count = value.get("count").and_then(|v| v.as_u64()).unwrap_or(0);
    let accession = value
        .get("accession")
        .and_then(|v| v.as_str())
        .unwrap_or("-");
    let files = value.get("files").and_then(|v| v.as_array());
    let errors = value.get("errors").and_then(|v| v.as_array());

    let mut out = format!("Downloaded **{count}** file(s) for **{accession}**\n\n");

    if let Some(files) = files {
        if !files.is_empty() {
            out.push_str("| File | Path | Size |\n");
            out.push_str("|------|------|------|\n");
            for f in files {
                let filename = str_or(f, "filename", "-");
                let path = str_or(f, "path", "-");
                let size = f.get("size").and_then(|v| v.as_u64()).unwrap_or(0);
                out.push_str(&format!(
                    "| {} | {} | {} |\n",
                    filename,
                    path,
                    format_bytes(size)
                ));
            }
        }
    }

    if let Some(errs) = errors {
        if !errs.is_empty() {
            out.push_str(&format!("\n**{} error(s):**\n\n", errs.len()));
            for e in errs {
                let file = str_or(e, "filename", "-");
                let msg = str_or(e, "error", "-");
                out.push_str(&format!("- **{file}**: {msg}\n"));
            }
        }
    }

    out
}

fn str_or<'a>(v: &'a serde_json::Value, key: &str, default: &'a str) -> &'a str {
    v.get(key).and_then(|v| v.as_str()).unwrap_or(default)
}

/// Format bytes into a human-readable string.
fn format_bytes(bytes: u64) -> String {
    if bytes == 0 {
        return "0 B".to_string();
    }
    const KB: u64 = 1024;
    const MB: u64 = 1024 * KB;
    const GB: u64 = 1024 * MB;
    if bytes >= GB {
        format!("{:.1} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.1} KB", bytes as f64 / KB as f64)
    } else {
        format!("{} B", bytes)
    }
}
