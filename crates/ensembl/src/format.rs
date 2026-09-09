//! Markdown summaries for Ensembl API responses.

use std::collections::BTreeMap;

use serde_json::Value;

use crate::sequence::nucleotide_stats;
use crate::types::*;

fn option(value: Option<&str>) -> &str {
    value.unwrap_or("-")
}

fn preview(sequence: &str, length: usize) -> String {
    if sequence.len() <= length {
        sequence.to_string()
    } else {
        format!(
            "{}...{}",
            &sequence[..length / 2],
            &sequence[sequence.len() - length / 2..]
        )
    }
}

/// Summarize a lookup entry and its child transcripts.
pub fn format_lookup(entry: &LookupEntry) -> String {
    let mut out = format!(
        "## {}\n\n- ID: `{}`\n- Object: {}\n- Biotype: {}\n- Species: {}\n- Location: `{}:{}:{}-{} (strand {})`\n- Assembly: {}\n",
        option(entry.display_name.as_deref()),
        entry.id,
        option(entry.object_type.as_deref()),
        option(entry.biotype.as_deref()),
        option(entry.species.as_deref()),
        entry.coordinates.assembly_name.as_deref().unwrap_or("-"),
        entry.coordinates.seq_region_name,
        entry.coordinates.start,
        entry.coordinates.end,
        entry.coordinates.strand,
        option(entry.coordinates.assembly_name.as_deref()),
    );
    if let Some(canonical) = &entry.canonical_transcript {
        out.push_str(&format!("- Canonical transcript: `{canonical}`\n"));
    }
    if let Some(description) = &entry.description {
        out.push_str(&format!("- Description: {description}\n"));
    }
    if !entry.transcripts.is_empty() {
        out.push_str(&format!("\n**Transcripts:** {}\n", entry.transcripts.len()));
        for transcript in entry.transcripts.iter().take(10) {
            out.push_str(&format!(
                "- `{}` {} {} ({} bp)\n",
                transcript.id,
                option(transcript.display_name.as_deref()),
                option(transcript.biotype.as_deref()),
                transcript.length.unwrap_or(
                    (transcript.coordinates.end - transcript.coordinates.start + 1).max(0) as u64
                ),
            ));
        }
    }
    out.trim_end().to_string()
}

/// Summarize sequence length and composition without dumping long sequences.
pub fn format_sequence(sequence: &Sequence) -> String {
    let mut out = format!(
        "## Sequence `{}`\n\n- Length: {}\n- Molecule: {}\n",
        sequence.query,
        sequence.seq.len(),
        option(sequence.molecule.as_deref()),
    );
    if let Some(desc) = &sequence.desc {
        out.push_str(&format!("- Region: {desc}\n"));
    }
    if sequence
        .molecule
        .as_deref()
        .is_some_and(|molecule| molecule.eq_ignore_ascii_case("dna"))
    {
        let stats = nucleotide_stats(&sequence.seq);
        out.push_str(&format!(
            "- Composition: A={}, C={}, G={}, T={}, N={}, other={}\n- GC fraction: {:.4}\n",
            stats.a, stats.c, stats.g, stats.t, stats.n, stats.other, stats.gc_fraction
        ));
    }
    out.push_str(&format!(
        "\n```text\n{}\n```\n",
        preview(&sequence.seq, 120)
    ));
    out.trim_end().to_string()
}

/// Summarize cross-references.
pub fn format_xrefs(xrefs: &[Xref], limit: usize) -> String {
    let mut out = format!("## Cross-references\n\nTotal: {}\n\n", xrefs.len());
    for xref in xrefs.iter().take(limit) {
        out.push_str(&format!(
            "- **{}** `{}` ({})\n",
            xref.db_display_name.as_deref().unwrap_or(&xref.dbname),
            xref.primary_id,
            xref.display_id.as_deref().unwrap_or("-"),
        ));
        if let Some(description) = xref.description.as_deref() {
            out.push_str(&format!("  - {description}\n"));
        }
    }
    if xrefs.len() > limit {
        out.push_str(&format!("\nShowing {limit} of {}.\n", xrefs.len()));
    }
    out.trim_end().to_string()
}

/// Summarize overlap features by type and coordinate span.
pub fn format_overlap(features: &[Value]) -> String {
    let mut counts = BTreeMap::new();
    for feature in features {
        let kind = feature
            .get("feature_type")
            .or_else(|| feature.get("object_type"))
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        *counts.entry(kind.to_string()).or_insert(0usize) += 1;
    }
    let mut out = format!("## Overlap features\n\nTotal: {}\n\n", features.len());
    for (kind, count) in counts {
        out.push_str(&format!("- {kind}: {count}\n"));
    }
    if let Some(first) = features.first() {
        out.push_str(&format!(
            "\nFirst feature:\n\n```json\n{}\n```\n",
            serde_json::to_string_pretty(first).unwrap_or_else(|_| "{}".to_string())
        ));
    }
    out.trim_end().to_string()
}

/// Summarize VEP consequences.
pub fn format_vep(results: &[VepResult]) -> String {
    let mut out = format!("## VEP results\n\nInputs annotated: {}\n", results.len());
    for result in results {
        out.push_str(&format!(
            "\n### `{}`\n\n- Consequence: {}\n- Alleles: {}\n- Location: `{}`\n- Transcripts: {}\n",
            result.input.as_deref().unwrap_or(&result.id),
            option(result.most_severe_consequence.as_deref()),
            option(result.allele_string.as_deref()),
            result
                .seq_region_name
                .as_deref()
                .map(|chromosome| format!(
                    "{chromosome}:{}-{}",
                    result.start.unwrap_or_default(),
                    result.end.unwrap_or_default()
                ))
                .unwrap_or_else(|| "-".to_string()),
            result.transcript_consequences.len(),
        ));
        for consequence in result.transcript_consequences.iter().take(8) {
            out.push_str(&format!(
                "- `{}` {} ({}){}\n",
                consequence.transcript_id,
                consequence.gene_symbol.as_deref().unwrap_or("-"),
                consequence.consequence_terms.join(", "),
                consequence
                    .impact
                    .as_deref()
                    .map(|impact| format!(" [{impact}]"))
                    .unwrap_or_default(),
            ));
        }
    }
    out.trim_end().to_string()
}

/// Summarize a variation record.
pub fn format_variation(variation: &Variation) -> String {
    let mut out = format!(
        "## Variation `{}`\n\n- Clinical significance: {}\n- Class: {}\n- Consequence: {}\n- Source: {}\n",
        variation.name,
        if variation.clinical_significance.is_empty() {
            "-".to_string()
        } else {
            variation.clinical_significance.join(", ")
        },
        option(variation.var_class.as_deref()),
        option(variation.most_severe_consequence.as_deref()),
        option(variation.source.as_deref()),
    );
    if let Some(maf) = variation.maf {
        out.push_str(&format!(
            "- MAF: {maf} ({})\n",
            option(variation.minor_allele.as_deref())
        ));
    }
    for mapping in &variation.mappings {
        out.push_str(&format!(
            "- Mapping: `{}` ({}, {})\n",
            mapping.location,
            mapping.assembly_name,
            mapping.allele_string.as_deref().unwrap_or("-")
        ));
    }
    out.trim_end().to_string()
}

/// Summarize a filtered species list.
pub fn format_species(species: &[Species]) -> String {
    let mut out = format!("## Ensembl species\n\nTotal: {}\n\n", species.len());
    for item in species.iter().take(15) {
        out.push_str(&format!(
            "- **{}** (`{}`, taxon {}, {} / {})\n",
            item.display_name.as_deref().unwrap_or(&item.name),
            item.name,
            item.taxon_id,
            item.assembly.as_deref().unwrap_or("-"),
            item.accession.as_deref().unwrap_or("-"),
        ));
    }
    if species.len() > 15 {
        out.push_str(&format!("\nShowing 15 of {}.\n", species.len()));
    }
    out.trim_end().to_string()
}

/// Summarize assembly metadata.
pub fn format_assembly(assembly: &AssemblyInfo) -> String {
    format!(
        "## Assembly {}\n\n- Accession: {}\n- Date: {}\n- Golden path: {}\n- Karyotype sequences: {}\n- Top-level regions: {}\n- Default coordinate system: {}\n",
        assembly.assembly_name,
        option(assembly.assembly_accession.as_deref()),
        option(assembly.assembly_date.as_deref()),
        assembly
            .golden_path
            .map(|value| value.to_string())
            .unwrap_or_else(|| "-".to_string()),
        assembly.karyotype.len(),
        assembly.top_level_region.len(),
        option(assembly.default_coord_system_version.as_deref()),
    )
}
