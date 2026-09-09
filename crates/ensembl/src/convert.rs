//! Column-oriented conversion helpers for DAG integration.
//!
//! [`Table`] intentionally uses JSON values rather than Arrow types. SDK
//! callers can inspect columns and cast each column when constructing a
//! RecordBatch, while the Ensembl crate remains independent of DataFusion.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::error::{EnsemblError, Result};
use crate::types::*;

/// A deterministic table representation with a fixed column order.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Table {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<Value>>,
}

impl Table {
    /// Build a table from arbitrary Ensembl JSON objects.
    ///
    /// Columns are ordered first by appearance and then by object key order.
    /// Nested objects and arrays are retained as compact JSON strings so every
    /// cell is scalar-shaped and can be mapped to an Arrow type by a caller.
    pub fn from_json_rows(rows: &[Value]) -> Result<Self> {
        if rows.iter().any(|row| !row.is_object()) {
            return Err(EnsemblError::InvalidParameter(
                "all Ensembl table rows must be JSON objects".to_string(),
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

fn scalarize(value: Value) -> Value {
    match value {
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => value,
        Value::Array(_) | Value::Object(_) => Value::String(value.to_string()),
    }
}

fn values<T: Serialize>(items: &[T]) -> Result<Vec<Value>> {
    items
        .iter()
        .map(serde_json::to_value)
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|e| EnsemblError::InvalidParameter(format!("failed to encode table row: {e}")))
}

/// Convert lookup entries into a generic table.
pub fn lookup_table(entries: &[LookupEntry]) -> Result<Table> {
    Table::from_json_rows(&values(entries)?)
}

/// Convert species metadata into a generic table.
pub fn species_table(species: &[Species]) -> Result<Table> {
    Table::from_json_rows(&values(species)?)
}

/// Convert assembly metadata into a one-row table.
pub fn assembly_table(assembly: &AssemblyInfo) -> Result<Table> {
    Table::from_json_rows(&[serde_json::to_value(assembly)?])
}

/// Convert cross-references into a generic table.
pub fn xrefs_table(xrefs: &[Xref]) -> Result<Table> {
    Table::from_json_rows(&values(xrefs)?)
}

/// Convert raw overlap features into a generic table.
pub fn overlap_table(features: &[Value]) -> Result<Table> {
    Table::from_json_rows(features)
}

/// Convert VEP results into one row per transcript consequence.
///
/// Results without transcript consequences still emit a row so callers retain
/// the input and top-level consequence.
pub fn vep_table(results: &[VepResult]) -> Result<Table> {
    let mut rows = Vec::new();
    for result in results {
        let consequences = &result.transcript_consequences;
        if consequences.is_empty() {
            rows.push(serde_json::to_value(result)?);
        } else {
            for consequence in consequences {
                let mut row = serde_json::to_value(result)?;
                let object = row.as_object_mut().ok_or_else(|| {
                    EnsemblError::InvalidParameter("VEP row must be a JSON object".to_string())
                })?;
                object.insert(
                    "transcript_consequence".to_string(),
                    serde_json::to_value(consequence)?,
                );
                object.remove("transcript_consequences");
                rows.push(row);
            }
        }
    }
    Table::from_json_rows(&rows)
}

/// Convert variation mappings into one row per mapping.
pub fn variation_table(variation: &Variation) -> Result<Table> {
    let mut rows = Vec::new();
    for mapping in &variation.mappings {
        rows.push(json!({
            "name": variation.name,
            "clinical_significance": variation.clinical_significance,
            "var_class": variation.var_class,
            "most_severe_consequence": variation.most_severe_consequence,
            "maf": variation.maf,
            "mapping": mapping,
        }));
    }
    Table::from_json_rows(&rows)
}
