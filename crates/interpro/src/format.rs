use crate::types::EntryResponse;

pub fn format_entry(response: &EntryResponse) -> String {
    let metadata = &response.metadata;
    let title = metadata
        .name
        .as_ref()
        .map(|name| name.name.as_str())
        .unwrap_or("InterPro entry");

    let mut output = format!(
        "## {title}\n\n- **Accession:** {}\n- **Type:** {}\n- **Database:** {}\n",
        metadata.accession,
        metadata.entry_type.as_deref().unwrap_or("unknown"),
        metadata.source_database.as_deref().unwrap_or("interpro"),
    );

    if let Some(short) = metadata
        .name
        .as_ref()
        .and_then(|name| name.short.as_deref())
    {
        output.push_str(&format!("- **Short name:** {short}\n"));
    }
    if let Some(counters) = &metadata.counters {
        output.push_str(&format!(
            "- **Matching proteins:** {}\n- **Structures:** {}\n",
            counters
                .proteins
                .map_or("unknown".into(), |value| value.to_string()),
            counters
                .structures
                .map_or("unknown".into(), |value| value.to_string()),
        ));
    }
    if let Some(structure) = &metadata.representative_structure {
        output.push_str(&format!(
            "- **Representative structure:** {} ({})\n",
            structure.accession, structure.name
        ));
    }
    if !metadata.go_terms.is_empty() {
        output.push_str("\n### GO terms\n\n");
        for term in &metadata.go_terms {
            output.push_str(&format!(
                "- `{}` — {}{}\n",
                term.identifier,
                term.name,
                term.category
                    .as_deref()
                    .map(|c| format!(" ({c})"))
                    .unwrap_or_default(),
            ));
        }
    }
    if !metadata.member_databases.is_empty() {
        output.push_str("\n### Member databases\n\n");
        for (database, entries) in &metadata.member_databases {
            let joined = entries
                .iter()
                .map(|(accession, name)| format!("`{accession}` {name}"))
                .collect::<Vec<_>>()
                .join(", ");
            output.push_str(&format!("- **{database}:** {joined}\n"));
        }
    }
    if let Some(first) = metadata
        .description
        .iter()
        .find_map(|item| item.text.clone())
    {
        let plain = first
            .replace("<p>", "")
            .replace("</p>", "\n")
            .replace("[[cite:", "[cite:")
            .replace("]]", "]");
        output.push_str(&format!("\n### Description\n\n{plain}\n"));
    }
    output
}
