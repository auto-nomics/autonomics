use crate::types::CompoundProperties;

pub fn format_compound(compound: &CompoundProperties, query: &str) -> String {
    let title = compound.title.as_deref().unwrap_or("PubChem compound");
    let mut output = format!(
        "## {title}\n\n- **PubChem CID:** {}\n",
        compound.cid.map_or("unknown".into(), |v| v.to_string())
    );
    output.push_str(&format!("- **Query:** {query}\n"));
    output.push_str(&format!(
        "- **Molecular formula:** {}\n",
        compound.molecular_formula.as_deref().unwrap_or("unknown")
    ));
    output.push_str(&format!(
        "- **Molecular weight:** {}\n",
        compound.molecular_weight.as_deref().unwrap_or("unknown")
    ));
    output.push_str(&format!(
        "- **IUPAC name:** {}\n",
        compound.iupac_name.as_deref().unwrap_or("unknown")
    ));
    output.push_str(&format!(
        "- **InChIKey:** {}\n",
        compound.inchikey.as_deref().unwrap_or("unknown")
    ));
    output.push_str(&format!(
        "- **SMILES:** `{}`\n",
        compound
            .smiles
            .as_deref()
            .or(compound.canonical_smiles.as_deref())
            .unwrap_or("not available")
    ));
    if let Some(inchi) = compound.inchi.as_deref() {
        output.push_str(&format!("- **InChI:** `{inchi}`\n"));
    }
    output
}
