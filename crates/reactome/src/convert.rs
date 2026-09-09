//! Column-oriented conversion helpers for DAG integration.
//!
//! [`Table`] intentionally uses JSON values rather than Arrow types. SDK
//! callers can inspect columns and cast each column when constructing a
//! RecordBatch, while the Reactome crate remains independent of DataFusion.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::{ReactomeError, Result};
use crate::types::*;

/// A deterministic table representation with a fixed column order.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Table {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<Value>>,
}

impl Table {
    /// Build a table from arbitrary Reactome JSON objects.
    ///
    /// Columns are ordered first by appearance and then by object key order.
    /// Nested objects and arrays are retained as compact JSON strings so
    /// every cell is scalar-shaped and can be mapped to an Arrow type by a
    /// caller.
    pub fn from_json_rows(rows: &[Value]) -> Result<Self> {
        if rows.iter().any(|row| !row.is_object()) {
            return Err(ReactomeError::InvalidParameter(
                "all Reactome table rows must be JSON objects".to_string(),
            ));
        }

        let mut columns: Vec<String> = Vec::new();
        for row in rows {
            for key in row.as_object().expect("checked JSON object").keys() {
                if !columns.iter().any(|column| column == key) {
                    columns.push(key.clone());
                }
            }
        }

        let rows = rows
            .iter()
            .map(|row| {
                columns
                    .iter()
                    .map(|column| row.get(column).cloned().map_or(Value::Null, scalarize))
                    .collect()
            })
            .collect();
        Ok(Self { columns, rows })
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

/// Reduce a JSON value to a scalar or a compact JSON string.
fn scalarize(value: Value) -> Value {
    match value {
        Value::Array(items) => Value::String(
            serde_json::to_string(&items).unwrap_or_else(|_| "[]".to_string()),
        ),
        Value::Object(_) => Value::String(
            serde_json::to_string(&value).unwrap_or_else(|_| "{}".to_string()),
        ),
        other => other,
    }
}

// ===========================================================================
// Pathway table
// ===========================================================================

/// Convert pathways into a columnar table.
pub fn pathways_to_table(pathways: &[Pathway]) -> Table {
    Table {
        columns: vec![
            "db_id".to_string(),
            "stable_id".to_string(),
            "display_name".to_string(),
            "schema_class".to_string(),
            "species_name".to_string(),
            "is_in_disease".to_string(),
            "is_inferred".to_string(),
            "has_diagram".to_string(),
            "has_ehld".to_string(),
        ],
        rows: pathways
            .iter()
            .map(|p| {
                vec![
                    serde_json::json!(p.db_id),
                    serde_json::json!(p.stable_id),
                    serde_json::json!(p.display_name),
                    serde_json::json!(p.schema_class),
                    serde_json::json!(p.species_name),
                    serde_json::json!(p.is_in_disease),
                    serde_json::json!(p.is_inferred),
                    serde_json::json!(p.has_diagram),
                    serde_json::json!(p.has_ehld),
                ]
            })
            .collect(),
    }
}

// ===========================================================================
// Analysis result table
// ===========================================================================

/// Convert enriched pathways into a columnar table.
pub fn analysis_to_table(result: &AnalysisResult) -> Table {
    Table {
        columns: vec![
            "stable_id".to_string(),
            "pathway_name".to_string(),
            "entities_found".to_string(),
            "entities_total".to_string(),
            "p_value".to_string(),
            "fdr".to_string(),
        ],
        rows: result
            .pathways
            .iter()
            .map(|p| {
                vec![
                    serde_json::json!(p.stable_id),
                    serde_json::json!(p.name),
                    serde_json::json!(p.entities.found),
                    serde_json::json!(p.entities.total),
                    serde_json::json!(p.entities.p_value),
                    serde_json::json!(p.entities.fdr),
                ]
            })
            .collect(),
    }
}
