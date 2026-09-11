use crate::types::Prediction;

pub fn format_predictions(predictions: &[Prediction], accession: &str) -> String {
    if predictions.is_empty() {
        return format!("No AlphaFold model was found for `{accession}`.");
    }

    let mut output = format!(
        "## AlphaFold models for {}\n\nFound {} model(s).\n",
        accession,
        predictions.len()
    );
    for prediction in predictions {
        output.push_str(&format!(
            "\n### {}\n\n- **UniProt accession:** {}\n- **Protein:** {}\n- **Gene:** {}\n- **Organism:** {}\n- **Latest version:** {}\n- **pLDDT metric:** {}\n- **Sequence range:** {}–{}\n- **CIF:** {}\n- **PDB:** {}\n- **Confidence JSON:** {}\n",
            prediction.entry_id.as_deref().unwrap_or("unknown"),
            prediction.uniprot_accession.as_deref().unwrap_or("unknown"),
            prediction.uniprot_description.as_deref().unwrap_or("unknown"),
            prediction.gene.as_deref().unwrap_or("unknown"),
            prediction.organism_scientific_name.as_deref().unwrap_or("unknown"),
            prediction.latest_version.map_or("unknown".into(), |v| v.to_string()),
            prediction.global_metric_value.map_or("unknown".into(), |v| v.to_string()),
            prediction.sequence_start.map_or("?".into(), |v| v.to_string()),
            prediction.sequence_end.map_or("?".into(), |v| v.to_string()),
            prediction.cif_url.as_deref().unwrap_or("not available"),
            prediction.pdb_url.as_deref().unwrap_or("not available"),
            prediction.plddt_doc_url.as_deref().unwrap_or("not available"),
        ));
    }
    output
}
