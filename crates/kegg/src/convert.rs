//! Deterministic row/table conversions for future DAG integration.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::types::{DatabaseInfo, DrugInteraction, EntrySummary, FlatEntry, Info, Pair};

/// A generic table that can be promoted to Arrow/DataFusion by a caller.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Table {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<Value>>,
}

impl Table {
    /// Construct a two-column table from stable string pairs.
    pub fn from_pairs(
        first: &str,
        second: &str,
        rows: impl IntoIterator<Item = (String, String)>,
    ) -> Self {
        Self {
            columns: vec![first.to_string(), second.to_string()],
            rows: rows
                .into_iter()
                .map(|(left, right)| vec![Value::from(left), Value::from(right)])
                .collect(),
        }
    }

    /// Number of rows.
    pub fn len(&self) -> usize {
        self.rows.len()
    }

    /// Whether the table has no rows.
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }
}

fn row(values: Vec<(&str, Value)>) -> Vec<Value> {
    values.into_iter().map(|(_, value)| value).collect()
}

/// Convert list/search summaries to rows.
pub fn entry_table(items: &[EntrySummary]) -> Table {
    Table::from_pairs(
        "id",
        "description",
        items
            .iter()
            .map(|item| (item.id.clone(), item.description.clone())),
    )
}

/// Convert conversion/link rows to rows.
pub fn pair_table(pairs: &[Pair]) -> Table {
    Table::from_pairs(
        "source",
        "target",
        pairs
            .iter()
            .map(|pair| (pair.source.clone(), pair.target.clone())),
    )
}

/// Convert parsed `info` metadata to database rows.
pub fn info_database_table(info: &Info) -> Table {
    let columns = vec![
        "database".to_string(),
        "entry_count".to_string(),
        "release_date".to_string(),
    ];
    let rows = info
        .databases
        .iter()
        .map(|item: &DatabaseInfo| {
            row(vec![
                ("database", Value::from(item.database.clone())),
                (
                    "entry_count",
                    item.entry_count.map(Value::from).unwrap_or(Value::Null),
                ),
                (
                    "release_date",
                    item.release_date
                        .clone()
                        .map(Value::from)
                        .unwrap_or(Value::Null),
                ),
            ])
        })
        .collect();
    Table { columns, rows }
}

/// Convert linked-database metadata to rows.
pub fn info_linked_table(info: &Info) -> Table {
    Table::from_pairs(
        "database",
        "linked_database",
        info.linked_databases
            .iter()
            .map(|linked| (info.title.clone(), linked.clone())),
    )
}

/// Convert DDI rows.
pub fn drug_interaction_table(interactions: &[DrugInteraction]) -> Table {
    let columns = vec![
        "drug".to_string(),
        "interacts_with".to_string(),
        "category".to_string(),
        "description".to_string(),
    ];
    let rows = interactions
        .iter()
        .map(|interaction| {
            row(vec![
                ("drug", Value::from(interaction.drug.clone())),
                (
                    "interacts_with",
                    Value::from(interaction.interacts_with.clone()),
                ),
                ("category", Value::from(interaction.category.clone())),
                ("description", Value::from(interaction.description.clone())),
            ])
        })
        .collect();
    Table { columns, rows }
}

/// Convert a flat-file entry header and full raw body into one row.
pub fn flat_entry_table(entry: &FlatEntry) -> Table {
    Table {
        columns: vec![
            "id".to_string(),
            "entry_type".to_string(),
            "organism".to_string(),
            "raw".to_string(),
        ],
        rows: vec![row(vec![
            ("id", Value::from(entry.id.clone())),
            (
                "entry_type",
                entry
                    .entry_type
                    .clone()
                    .map(Value::from)
                    .unwrap_or(Value::Null),
            ),
            (
                "organism",
                entry
                    .organism
                    .clone()
                    .map(Value::from)
                    .unwrap_or(Value::Null),
            ),
            ("raw", Value::from(entry.raw.clone())),
        ])],
    }
}
