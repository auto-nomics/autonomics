//! Parsers for KEGG's tab-delimited and flat-file response formats.

use crate::types::{DatabaseInfo, DrugInteraction, EntrySummary, FlatEntry, Info, Pair};

fn split_line(line: &str) -> (String, String) {
    match line.split_once('\t') {
        Some((id, description)) => (id.trim().to_string(), description.trim().to_string()),
        None => (line.trim().to_string(), String::new()),
    }
}

/// Parse `list` or `find` output.
pub fn entry_summaries(text: &str) -> Vec<EntrySummary> {
    text.lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            let (id, description) = split_line(line);
            EntrySummary { id, description }
        })
        .collect()
}

/// Parse `conv` or `link` output.
pub fn pairs(text: &str) -> Vec<Pair> {
    entry_summaries(text)
        .into_iter()
        .filter(|summary| !summary.id.is_empty())
        .map(|summary| Pair {
            source: summary.id,
            target: summary.description,
        })
        .collect()
}

/// Parse `ddi` output.
pub fn drug_interactions(text: &str) -> Vec<DrugInteraction> {
    text.lines()
        .filter(|line| !line.trim().is_empty())
        .filter_map(|line| {
            let fields: Vec<_> = line.split('\t').map(str::trim).collect();
            match fields[..] {
                [drug, interacts_with, category, description, ..] => Some(DrugInteraction {
                    drug: drug.to_string(),
                    interacts_with: interacts_with.to_string(),
                    category: category.to_string(),
                    description: description.to_string(),
                }),
                _ => None,
            }
        })
        .collect()
}

/// Parse database metadata from `info` while retaining the original text.
pub fn info(text: &str) -> Info {
    let mut title = String::new();
    let mut summary_lines = Vec::new();
    let mut databases = Vec::new();
    let mut linked_databases = Vec::new();
    let mut in_linked = false;

    for line in text.lines() {
        if line.trim().is_empty() {
            continue;
        }
        if title.is_empty() {
            title = line.trim().to_string();
        }

        if line.starts_with('\t') || line.starts_with("    ") {
            let value = line.trim();
            if in_linked {
                linked_databases.push(value.to_string());
                continue;
            }
            let fields: Vec<_> = value.split('\t').map(str::trim).collect();
            let database = fields.first().copied().unwrap_or_default();
            let count = fields
                .get(1)
                .and_then(|value| value.replace(',', "").parse::<u64>().ok());
            let release_date = fields.get(2).map(|value| value.to_string());
            if !database.is_empty() {
                databases.push(DatabaseInfo {
                    database: database.to_string(),
                    entry_count: count,
                    release_date,
                });
            }
            continue;
        }

        if line.trim().eq_ignore_ascii_case("linked db") {
            in_linked = true;
            continue;
        }
        if !in_linked {
            summary_lines.push(line.trim_end().to_string());
        }
    }

    Info {
        title,
        summary: summary_lines.join("\n"),
        databases,
        linked_databases,
        raw: text.to_string(),
    }
}

/// Parse the `ENTRY` header from a flat-file response.
pub fn flat_entry(raw: &str) -> FlatEntry {
    let header = raw.lines().find(|line| line.starts_with("ENTRY "));
    let mut fields = header
        .unwrap_or_default()
        .split_whitespace()
        .skip(1)
        .map(str::to_string);
    let id = fields.next().unwrap_or_default();
    let entry_type = fields.next().filter(|value| !value.starts_with('T'));
    let organism = fields.next().filter(|value| value.starts_with('T'));

    FlatEntry {
        id,
        entry_type,
        organism,
        raw: raw.to_string(),
    }
}
