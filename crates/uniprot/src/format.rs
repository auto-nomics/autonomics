//! Render UniProt API responses into clean, LLM-friendly Markdown.

use crate::types::*;

// ===========================================================================
// UniProtKB search results
// ===========================================================================

/// Format one page of UniProtKB search results as a Markdown preview list.
pub fn format_entries(page: &SearchResults<Entry>) -> String {
    let mut out = String::with_capacity(8192);

    match page.total_results {
        Some(total) => out.push_str(&format!("**{total}** entries found")),
        None => out.push_str("Entries"),
    }
    let n = page.results.len();
    if n > 0 {
        out.push_str(&format!(" (showing {n})\n\n"));
    } else {
        out.push_str("\n\nNo entries returned.\n");
        return out;
    }

    for (i, entry) in page.results.iter().enumerate() {
        format_entry_preview(i + 1, entry, &mut out);
        out.push('\n');
    }

    if let Some(ref cursor) = page.next_cursor {
        out.push_str(&format!("_Next page cursor:_ `{cursor}`\n\n"));
    }
    if let Some(ref release) = page.release {
        out.push_str(&format!("_UniProt release:_ {release}\n"));
    }

    out.trim_end().to_string()
}

/// Format a single entry with full detail (function, keywords, …).
pub fn format_entry(entry: &Entry) -> String {
    let mut out = String::with_capacity(8192);
    format_entry_full(entry, &mut out);
    out.trim_end().to_string()
}

fn format_entry_preview(i: usize, entry: &Entry, out: &mut String) {
    out.push_str(&format!(
        "### {}. {} {}\n",
        i, entry.primary_accession, entry.uni_protkb_id
    ));

    out.push_str(&format!(
        "**Type:** {}\n",
        if entry.is_reviewed() {
            "reviewed (Swiss-Prot)"
        } else {
            "unreviewed (TrEMBL)"
        }
    ));

    if let Some(name) = entry.protein_name() {
        out.push_str(&format!("**Protein:** {name}\n"));
    }

    let genes = entry.gene_names();
    if !genes.is_empty() {
        out.push_str(&format!("**Genes:** {}\n", genes.join(", ")));
    }

    if let Some(ref organism) = organism_display(entry) {
        out.push_str(&format!("**Organism:** {organism}\n"));
    }

    if let Some(len) = entry.sequence.length {
        out.push_str(&format!("**Length:** {len} aa\n"));
    }

    if let Some(func) = entry.function_text() {
        out.push_str(&format!("\n> {}\n", truncate_at(&func, 300)));
    }

    out.push('\n');
}

fn format_entry_full(entry: &Entry, out: &mut String) {
    out.push_str(&format!(
        "## {} {}\n\n",
        entry.primary_accession, entry.uni_protkb_id
    ));

    out.push_str(&format!(
        "**Type:** {}\n",
        if entry.is_reviewed() {
            "reviewed (Swiss-Prot)"
        } else {
            "unreviewed (TrEMBL)"
        }
    ));
    if let Some(pe) = &entry.protein_existence {
        out.push_str(&format!("**Protein existence:** {pe}\n"));
    }
    if let Some(score) = entry.annotation_score {
        out.push_str(&format!("**Annotation score:** {score}/5\n"));
    }

    if let Some(name) = entry.protein_name() {
        out.push_str(&format!("**Protein:** {name}\n"));
    }
    let alt: Vec<&str> = entry
        .protein_description
        .alternative_names
        .iter()
        .filter_map(|n| n.full_name.as_ref().map(|s| s.value.as_str()))
        .collect();
    if !alt.is_empty() {
        out.push_str(&format!("**Also known as:** {}\n", alt.join("; ")));
    }

    let genes = entry.gene_names();
    if !genes.is_empty() {
        out.push_str(&format!("**Genes:** {}\n", genes.join(", ")));
    }

    out.push_str(&format!(
        "**Organism:** {} (taxon {})\n",
        organism_display(entry).as_deref().unwrap_or("?"),
        entry.organism.taxon_id
    ));

    if let Some(len) = entry.sequence.length {
        out.push_str(&format!("**Length:** {len} aa"));
        if let Some(mw) = entry.sequence.mol_weight {
            out.push_str(&format!(", {mw} Da"));
        }
        out.push('\n');
    }

    if !entry.comments.is_empty() {
        out.push_str("\n### Comments\n\n");
        for comment in &entry.comments {
            let text = comment.text();
            if text.is_empty() {
                continue;
            }
            out.push_str(&format!(
                "- **{}:** {}\n",
                title_case(&comment.comment_type),
                text
            ));
        }
    }

    if !entry.keywords.is_empty() {
        let kws: Vec<&str> = entry.keywords.iter().map(|k| k.name.as_str()).collect();
        out.push_str(&format!("\n**Keywords:** {}\n", kws.join("; ")));
    }

    if !entry.uni_protkb_cross_references.is_empty() {
        out.push_str("\n### Cross-references\n\n");
        out.push_str("| Database | ID |\n|----------|----|\n");
        for xref in entry.uni_protkb_cross_references.iter().take(50) {
            out.push_str(&format!("| {} | {} |\n", xref.database, xref.id));
        }
        let n = entry.uni_protkb_cross_references.len();
        if n > 50 {
            out.push_str(&format!("| … | ({n} total) |\n"));
        }
    }

    if !entry.references.is_empty() {
        out.push_str(&format!("\n**References:** {}\n", entry.references.len()));
    }
}

// ===========================================================================
// Taxonomy
// ===========================================================================

/// Format one page of taxonomy search results as Markdown.
pub fn format_taxa(page: &SearchResults<Taxon>) -> String {
    let mut out = String::with_capacity(4096);

    match page.total_results {
        Some(total) => out.push_str(&format!("**{total}** taxa found")),
        None => out.push_str("Taxa"),
    }
    if page.results.is_empty() {
        out.push_str("\n\nNo taxa returned.\n");
        return out;
    }
    out.push_str("\n\n");

    for taxon in &page.results {
        out.push_str(&format!("### {} ({})\n", taxon.name(), taxon.taxon_id));
        if let Some(ref rank) = taxon.rank {
            out.push_str(&format!("**Rank:** {rank}\n"));
        }
        if let Some(ref mnemonic) = taxon.mnemonic {
            out.push_str(&format!("**Mnemonic:** {mnemonic}\n"));
        }
        if let Some(ref parent) = taxon.parent {
            out.push_str(&format!(
                "**Parent:** {} ({})\n",
                parent.scientific_name.as_deref().unwrap_or("?"),
                parent.taxon_id
            ));
        }
        let lineage: Vec<&str> = taxon
            .lineage
            .iter()
            .filter_map(|n| n.scientific_name.as_deref())
            .collect();
        if !lineage.is_empty() {
            out.push_str(&format!("**Lineage:** {}\n", lineage.join(" > ")));
        }
        out.push('\n');
    }

    if let Some(ref cursor) = page.next_cursor {
        out.push_str(&format!("_Next page cursor:_ `{cursor}`\n\n"));
    }

    out.trim_end().to_string()
}

// ===========================================================================
// Proteomes
// ===========================================================================

/// Format one page of proteome search results as a Markdown table.
pub fn format_proteomes(page: &SearchResults<Proteome>) -> String {
    let mut out = String::with_capacity(4096);

    match page.total_results {
        Some(total) => out.push_str(&format!("**{total}** proteomes found")),
        None => out.push_str("Proteomes"),
    }
    if page.results.is_empty() {
        out.push_str("\n\nNo proteomes returned.\n");
        return out;
    }
    out.push_str("\n\n");

    out.push_str(
        "| UPID | Organism | Taxon | Type | Proteins (reviewed) |\n|------|----------|-------|------|--------------------|\n",
    );
    for proteome in &page.results {
        let organism = proteome.taxonomy.scientific_name.as_deref().unwrap_or("?");
        let proteins = match proteome.proteome_statistics.reviewed_protein_count {
            Some(reviewed) => format!("{} ({})", proteome.protein_count.unwrap_or(0), reviewed),
            None => proteome.protein_count.unwrap_or(0).to_string(),
        };
        out.push_str(&format!(
            "| {} | {} | {} | {} | {} |\n",
            proteome.id,
            organism,
            proteome.taxonomy.taxon_id,
            proteome.proteome_type.as_deref().unwrap_or("-"),
            proteins
        ));
    }

    if let Some(ref cursor) = page.next_cursor {
        out.push_str(&format!("\n_Next page cursor:_ `{cursor}`\n"));
    }

    out.trim_end().to_string()
}

// ===========================================================================
// ID mapping
// ===========================================================================

/// Format typed ID mapping results as a Markdown table.
pub fn format_id_mapping(results: &IdMappingResults) -> String {
    let mut out = String::with_capacity(2048);

    let n = results.results.len();
    if n == 0 {
        return "No mapping results.\n".to_string();
    }
    out.push_str(&format!("**{n}** mapped IDs\n\n"));
    out.push_str("| From | To | Entry name | Protein |\n|------|----|------------|---------|\n");
    for row in &results.results {
        match &row.to {
            Some(entry) => out.push_str(&format!(
                "| {} | {} | {} | {} |\n",
                row.from,
                entry.primary_accession,
                entry.uni_protkb_id,
                entry.protein_name().unwrap_or("-")
            )),
            None => out.push_str(&format!("| {} | – | – unmapped – | |\n", row.from)),
        }
    }

    out.trim_end().to_string()
}

// ===========================================================================
// Helpers
// ===========================================================================

/// Display organism as scientific name, falling back to common name.
fn organism_display(entry: &Entry) -> Option<String> {
    entry
        .organism
        .scientific_name
        .clone()
        .or_else(|| entry.organism.common_name.clone())
}

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

/// `"SUBCELLULAR LOCATION"` → `"Subcellular location"`.
fn title_case(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut at_word_start = true;
    for ch in s.chars() {
        let lowered = ch.to_ascii_lowercase();
        if at_word_start && lowered.is_ascii_alphabetic() {
            out.push(lowered.to_ascii_uppercase());
        } else {
            out.push(lowered);
        }
        at_word_start = !lowered.is_ascii_alphanumeric();
    }
    out
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn entry_fixture() -> Entry {
        serde_json::from_str(
            r#"{
              "entryType": "UniProtKB reviewed (Swiss-Prot)",
              "primaryAccession": "P01308",
              "uniProtkbId": "INS_HUMAN",
              "organism": {"scientificName": "Homo sapiens", "commonName": "Human", "taxonId": 9606},
              "proteinDescription": {"recommendedName": {"fullName": {"value": "Insulin"}}},
              "genes": [{"geneName": {"value": "INS"}}],
              "comments": [
                {"commentType": "FUNCTION", "texts": [{"value": "Insulin lowers blood glucose levels."}]}
              ],
              "keywords": [{"id": "KW-0002", "category": "Technical term", "name": "3D-structure"}],
              "uniProtKBCrossReferences": [{"database": "PDB", "id": "1A7F"}],
              "sequence": {"value": "MALWMRLL", "length": 110, "molWeight": 11135}
            }"#,
        )
        .unwrap()
    }

    #[test]
    fn format_entries_basic() {
        let page = SearchResults {
            results: vec![entry_fixture()],
            total_results: Some(42),
            next_cursor: Some("cur123".into()),
            release: Some("2026_03".into()),
        };
        let rendered = format_entries(&page);
        assert!(rendered.contains("**42** entries found"));
        assert!(rendered.contains("### 1. P01308 INS_HUMAN"));
        assert!(rendered.contains("reviewed (Swiss-Prot)"));
        assert!(rendered.contains("**Protein:** Insulin"));
        assert!(rendered.contains("**Genes:** INS"));
        assert!(rendered.contains("**Organism:** Homo sapiens"));
        assert!(rendered.contains("**Length:** 110 aa"));
        assert!(rendered.contains("Insulin lowers blood glucose levels."));
        assert!(rendered.contains("`cur123`"));
        assert!(rendered.contains("2026_03"));
    }

    #[test]
    fn format_entries_empty() {
        let page = SearchResults::<Entry>::default();
        let rendered = format_entries(&page);
        assert!(rendered.contains("No entries returned"));
    }

    #[test]
    fn format_entry_full_detail() {
        let rendered = format_entry(&entry_fixture());
        assert!(rendered.contains("## P01308 INS_HUMAN"));
        assert!(rendered.contains("110 aa, 11135 Da"));
        assert!(rendered.contains("### Comments"));
        assert!(rendered.contains("- **Function:** Insulin lowers"));
        assert!(rendered.contains("**Keywords:** 3D-structure"));
        assert!(rendered.contains("| PDB | 1A7F |"));
    }

    #[test]
    fn format_taxa_basic() {
        let page = SearchResults {
            results: vec![Taxon {
                taxon_id: 9606,
                scientific_name: Some("Homo sapiens".into()),
                common_name: Some("Human".into()),
                rank: Some("species".into()),
                parent: Some(TaxonNode {
                    taxon_id: 9605,
                    scientific_name: Some("Homo".into()),
                    ..Default::default()
                }),
                ..Default::default()
            }],
            total_results: Some(1),
            next_cursor: None,
            release: None,
        };
        let rendered = format_taxa(&page);
        assert!(rendered.contains("**1** taxa found"));
        assert!(rendered.contains("### Homo sapiens (9606)"));
        assert!(rendered.contains("**Rank:** species"));
        assert!(rendered.contains("**Parent:** Homo (9605)"));
    }

    #[test]
    fn format_proteomes_table() {
        let page = SearchResults {
            results: vec![Proteome {
                id: "UP000005640".into(),
                taxonomy: TaxonSummary {
                    scientific_name: Some("Homo sapiens".into()),
                    taxon_id: 9606,
                    ..Default::default()
                },
                proteome_type: Some("Reference proteome".into()),
                protein_count: Some(81973),
                proteome_statistics: ProteomeStatistics {
                    reviewed_protein_count: Some(20401),
                    ..Default::default()
                },
                ..Default::default()
            }],
            total_results: Some(1),
            next_cursor: None,
            release: None,
        };
        let rendered = format_proteomes(&page);
        assert!(rendered.contains("**1** proteomes found"));
        assert!(rendered.contains("| UP000005640 | Homo sapiens | 9606 |"));
        assert!(rendered.contains("81973 (20401)"));
    }

    #[test]
    fn format_id_mapping_table() {
        let results = IdMappingResults {
            results: vec![
                IdMappingResult {
                    from: "INS".into(),
                    to: Some(entry_fixture()),
                },
                IdMappingResult {
                    from: "NOPE".into(),
                    to: None,
                },
            ],
        };
        let rendered = format_id_mapping(&results);
        assert!(rendered.contains("**2** mapped IDs"));
        assert!(rendered.contains("| INS | P01308 | INS_HUMAN | Insulin |"));
        assert!(rendered.contains("| NOPE | – | – unmapped – | |"));
    }

    #[test]
    fn title_case_works() {
        assert_eq!(title_case("FUNCTION"), "Function");
        assert_eq!(title_case("SUBCELLULAR LOCATION"), "Subcellular Location");
        assert_eq!(title_case("PTM"), "Ptm");
    }
}
