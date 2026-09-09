//! Concise text summaries for agent-facing KEGG tools.

use crate::types::{DrugInteraction, EntrySummary, FlatEntry, Info, Pair};

fn option(value: Option<&str>) -> &str {
    value.unwrap_or("-")
}

/// Render database or organism metadata.
pub fn format_info(info: &Info) -> String {
    let mut out = format!("## KEGG info: {}\n\n", info.title);
    if !info.summary.is_empty() {
        out.push_str(&info.summary);
        out.push_str("\n\n");
    }
    if !info.databases.is_empty() {
        out.push_str(&format!("**Databases:** {}\n", info.databases.len()));
        for database in info.databases.iter().take(25) {
            let count = database
                .entry_count
                .map(|value| value.to_string())
                .unwrap_or_else(|| "-".to_string());
            let date = option(database.release_date.as_deref());
            out.push_str(&format!(
                "- `{}`: {} ({})\n",
                database.database, count, date
            ));
        }
        if info.databases.len() > 25 {
            out.push_str(&format!(
                "\nShowing 25 of {} databases.\n",
                info.databases.len()
            ));
        }
    }
    if !info.linked_databases.is_empty() {
        out.push_str(&format!(
            "\n**Linked databases:** {}\n",
            info.linked_databases.join(", ")
        ));
    }
    out.trim_end().to_string()
}

/// Render a bounded preview of list/search results.
pub fn format_entries(items: &[EntrySummary], limit: usize) -> String {
    let shown = limit.min(items.len());
    let mut out = format!("## KEGG entries\n\nTotal: {}\n\n", items.len());
    for item in items.iter().take(shown) {
        out.push_str(&format!("- `{}` {}\n", item.id, item.description));
    }
    if items.len() > shown {
        out.push_str(&format!("\nShowing {shown} of {} entries.\n", items.len()));
    }
    out.trim_end().to_string()
}

/// Render relationship rows such as gene/pathway links or ID conversions.
pub fn format_pairs(title: &str, pairs: &[Pair], limit: usize) -> String {
    let shown = limit.min(pairs.len());
    let mut out = format!("## {title}\n\nTotal: {}\n\n", pairs.len());
    for pair in pairs.iter().take(shown) {
        out.push_str(&format!("- `{}` -> `{}`\n", pair.source, pair.target));
    }
    if pairs.len() > shown {
        out.push_str(&format!("\nShowing {shown} of {} rows.\n", pairs.len()));
    }
    out.trim_end().to_string()
}

/// Render a short entry preview without emitting the full flat file.
pub fn format_entry(entry: &FlatEntry, limit: usize) -> String {
    let mut out = format!(
        "## KEGG entry `{}`\n\n- Type: {}\n- Organism/genome: {}\n",
        entry.id,
        option(entry.entry_type.as_deref()),
        option(entry.organism.as_deref()),
    );

    let selected: Vec<&str> = entry
        .raw
        .lines()
        .filter(|line| {
            [
                "SYMBOL",
                "NAME",
                "ORTHOLOGY",
                "ORGANISM",
                "PATHWAY",
                "DISEASE",
                "DBLINKS",
            ]
            .iter()
            .any(|prefix| line.starts_with(prefix))
        })
        .take(limit)
        .collect();
    if !selected.is_empty() {
        out.push_str("\n```text\n");
        out.push_str(&selected.join("\n"));
        out.push_str("\n```\n");
    }
    out.push_str(&format!(
        "\nFlat-file length: {} bytes, {} lines.\n",
        entry.raw.len(),
        entry.raw.lines().count()
    ));
    out.trim_end().to_string()
}

/// Render raw text with a hard preview cap.
pub fn format_raw(title: &str, raw: &str, limit: usize) -> String {
    let lines: Vec<&str> = raw.lines().take(limit).collect();
    let mut out = format!("## {title}\n\n```text\n{}\n```\n", lines.join("\n"));
    let total = raw.lines().count();
    if total > limit {
        out.push_str(&format!("\nShowing {limit} of {total} lines.\n"));
    }
    out.trim_end().to_string()
}

/// Render drug-drug interaction rows.
pub fn format_drug_interactions(interactions: &[DrugInteraction], limit: usize) -> String {
    let shown = limit.min(interactions.len());
    let mut out = format!("## Drug interactions\n\nTotal: {}\n\n", interactions.len());
    for interaction in interactions.iter().take(shown) {
        out.push_str(&format!(
            "- `{}` + `{}`: [{}] {}\n",
            interaction.drug,
            interaction.interacts_with,
            interaction.category,
            interaction.description
        ));
    }
    if interactions.len() > shown {
        out.push_str(&format!(
            "\nShowing {shown} of {} interactions.\n",
            interactions.len()
        ));
    }
    out.trim_end().to_string()
}
