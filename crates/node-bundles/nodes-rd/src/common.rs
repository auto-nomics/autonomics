//! Shared helpers for RD nodes: column extraction from RecordBatches.

use arrow_array::{Array, Float64Array, Int64Array, RecordBatch, StringArray};
use arrow_schema::DataType;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum RdNodeError {
    #[error("missing column '{name}' in input DataFrame")]
    MissingColumn { name: String },
    #[error("column '{name}' is not numeric (got {dtype})")]
    WrongColumnType { name: String, dtype: String },
    #[error("no input data: expected at least one row")]
    EmptyInput,
    #[error("RD computation failed: {0}")]
    Compute(String),
    #[error("failed to build result batch: {0}")]
    Arrow(#[from] arrow_schema::ArrowError),
    #[error("failed to read result batch: {0}")]
    ReadBatch(#[from] datafusion::error::DataFusionError),
}

impl ::dag_core::dag::NodeError for RdNodeError {
    fn node_type(&self) -> &str {
        "rd_node"
    }
}

/// Extract a numeric column as `Vec<f64>`.
pub fn extract_f64(batches: &[RecordBatch], name: &str) -> Result<Vec<f64>, RdNodeError> {
    let schema = batches
        .first()
        .map(|b| b.schema().clone())
        .ok_or(RdNodeError::EmptyInput)?;
    let idx = schema
        .index_of(name)
        .map_err(|_| RdNodeError::MissingColumn { name: name.into() })?;
    let mut out = Vec::new();
    for batch in batches {
        out.extend(numeric_values(batch.column(idx)));
    }
    Ok(out)
}

/// Extract a numeric column as `Vec<f64>` if present.
pub fn extract_opt_f64(batches: &[RecordBatch], name: &str) -> Option<Vec<f64>> {
    extract_f64(batches, name).ok()
}

/// Extract numeric values from an Arrow array, handling all numeric types.
pub fn numeric_values(col: &dyn Array) -> Vec<f64> {
    let mut out = Vec::with_capacity(col.len());
    macro_rules! cast {
        ($T:ty) => {
            if let Some(a) = col.as_any().downcast_ref::<$T>() {
                for v in a.iter() {
                    out.push(match v {
                        Some(x) => x as f64,
                        None => f64::NAN,
                    });
                }
                return out;
            }
        };
    }
    cast!(arrow_array::Int8Array);
    cast!(arrow_array::Int16Array);
    cast!(arrow_array::Int32Array);
    cast!(arrow_array::Int64Array);
    cast!(arrow_array::UInt8Array);
    cast!(arrow_array::UInt16Array);
    cast!(arrow_array::UInt32Array);
    cast!(arrow_array::UInt64Array);
    cast!(arrow_array::Float32Array);
    cast!(Float64Array);
    for _ in 0..col.len() {
        out.push(f64::NAN);
    }
    out
}
