use crate::search::SearchResponse;
use crate::types::{Entry, PolymerEntity};

pub fn format_entry(entry: &Entry) -> String {
    let mut out = String::with_capacity(4096);
    out.push_str(&format!(
        "## {id}: {title}\n\n",
        id = entry.rcsb_id,
        title = entry.title()
    ));

    if let Some(method) = entry.experimental_method() {
        out.push_str(&format!("- **Method:** {method}\n"));
    }
    if let Some(resolution) = entry.resolution() {
        out.push_str(&format!("- **Resolution:** {resolution:.2} A\n"));
    }
    if let Some(count) = entry.rcsb_entry_info.deposited_atom_count {
        out.push_str(&format!("- **Deposited atoms:** {count}\n"));
    }
    if let Some(count) = entry.rcsb_entry_info.polymer_entity_count {
        out.push_str(&format!("- **Polymer entities:** {count}\n"));
    }
    if let Some(weight) = entry.rcsb_entry_info.molecular_weight {
        out.push_str(&format!("- **Molecular weight:** {weight:.2} kDa\n"));
    }
    if let Some(space_group) = &entry.symmetry.space_group_name_h_m {
        out.push_str(&format!("- **Space group:** {space_group}\n"));
    }
    if !entry.author_string().is_empty() {
        out.push_str(&format!("- **Authors:** {}\n", entry.author_string()));
    }

    if let Some(citation) = entry.primary_citation() {
        out.push_str("\n### Primary citation\n\n");
        out.push_str(&format!("- **Title:** {}\n", citation.title));
        if let Some(year) = citation.year {
            out.push_str(&format!("- **Year:** {year}\n"));
        }
        if let Some(journal) = &citation.rcsb_journal_abbrev {
            out.push_str(&format!("- **Journal:** {journal}\n"));
        }
        if let Some(doi) = &citation.doi {
            out.push_str(&format!("- **DOI:** {doi}\n"));
        }
        if let Some(pubmed) = citation.pubmed_id {
            out.push_str(&format!("- **PubMed:** {pubmed}\n"));
        }
    }

    out.trim_end().to_owned()
}

pub fn format_search_summaries(response: &SearchResponse, entries: &[Entry]) -> String {
    let mut out = format!(
        "**{}** results found (showing {} identifiers)\n\n",
        response.total_count,
        response.result_set.len()
    );

    if entries.is_empty() {
        for (index, result) in response.result_set.iter().enumerate() {
            out.push_str(&format!(
                "{}. `{}` — score {:.3}\n",
                index + 1,
                result.identifier,
                result.score
            ));
        }
        return out;
    }

    for (index, entry) in entries.iter().enumerate() {
        out.push_str(&format!(
            "### {}. {} — {}\n",
            index + 1,
            entry.rcsb_id,
            entry.title()
        ));
        if let Some(method) = entry.experimental_method() {
            out.push_str(&format!("- Method: {method}\n"));
        }
        if let Some(resolution) = entry.resolution() {
            out.push_str(&format!("- Resolution: {resolution:.2} A\n"));
        }
        if let Some(count) = entry.rcsb_entry_info.polymer_entity_count {
            out.push_str(&format!("- Polymer entities: {count}\n"));
        }
        out.push('\n');
    }

    out.trim_end().to_owned()
}

pub fn format_polymer_entity(entity: &PolymerEntity) -> String {
    let mut out = format!(
        "## {id}: {description}\n\n",
        id = entity.rcsb_id,
        description = entity.description()
    );
    out.push_str(&format!(
        "- **Length:** {} residues\n",
        entity.sequence_length()
    ));
    if let Some(polymer_type) = &entity.entity_poly.rcsb_entity_polymer_type {
        out.push_str(&format!("- **Polymer type:** {polymer_type}\n"));
    }
    if let Some(copies) = entity.rcsb_polymer_entity.pdbx_number_of_molecules {
        out.push_str(&format!("- **Copies in entry:** {copies}\n"));
    }
    if !entity
        .rcsb_polymer_entity_container_identifiers
        .uniprot_ids
        .is_empty()
    {
        out.push_str(&format!(
            "- **UniProt:** {}\n",
            entity
                .rcsb_polymer_entity_container_identifiers
                .uniprot_ids
                .join(", ")
        ));
    }
    if let Some(organism) = entity.first_source_organism() {
        if let Some(name) = &organism.scientific_name {
            out.push_str(&format!("- **Organism:** {name}"));
            if let Some(taxonomy_id) = organism.ncbi_taxonomy_id {
                out.push_str(&format!(" (taxon {taxonomy_id})"));
            }
            out.push('\n');
        }
        if !organism.rcsb_gene_name.is_empty() {
            let genes = organism
                .rcsb_gene_name
                .iter()
                .map(|gene| gene.value.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            out.push_str(&format!("- **Genes:** {genes}\n"));
        }
    }

    let sequence = entity.sequence();
    let preview: String = sequence.chars().take(120).collect();
    let suffix = if sequence.chars().count() > 120 {
        "..."
    } else {
        ""
    };
    out.push_str(&format!("\n```text\n{preview}{suffix}\n```\n"));
    out.trim_end().to_owned()
}

pub fn format_structure_preview(
    entry_id: &str,
    format: &str,
    url: &str,
    bytes: usize,
    text: &str,
) -> String {
    let lines = text.lines().filter(|line| !line.trim().is_empty()).take(40);
    let mut preview = String::new();
    for line in lines {
        preview.push_str(line);
        preview.push('\n');
    }
    format!(
        "## {entry_id} structure preview\n\n- **Format:** {format}\n- **URL:** {url}\n- **Size:** {bytes} bytes\n\n```text\n{preview}\n```"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entry_formatter_contains_core_metadata() {
        let entry: Entry = serde_json::from_str(
            r#"{
              "rcsb_id": "4HHB",
              "struct": {"title": "Human hemoglobin"},
              "exptl": [{"method": "X-RAY DIFFRACTION"}],
              "rcsb_entry_info": {"deposited_atom_count": 4779, "resolution_combined": [1.74]}
            }"#,
        )
        .unwrap();
        let markdown = format_entry(&entry);
        assert!(markdown.contains("X-RAY DIFFRACTION"));
        assert!(markdown.contains("4779"));
    }
}
