//! Common helpers for MICE node implementations.

use std::sync::Arc;

use arrow_array::{
    Array, Float32Array, Float64Array, Int8Array, Int16Array, Int32Array, Int64Array, RecordBatch,
    UInt8Array, UInt16Array, UInt32Array, UInt64Array,
};
use arrow_schema::{DataType, Schema, SchemaRef};
use dag_core::registry::NodeCtx;

use crate::error::MiceNodeError;

/// Pull a numeric column from the upstream batches as `Vec<f64>`.
pub fn extract_f64(batches: &[RecordBatch], name: &str) -> Result<Vec<f64>, MiceNodeError> {
    let schema = batches
        .first()
        .map(|b| b.schema().clone())
        .ok_or_else(|| MiceNodeError::EmptyInput)?;
    let idx = schema
        .index_of(name)
        .map_err(|_| MiceNodeError::MissingColumn {
            name: name.to_string(),
        })?;
    let col = &batches[0].column(idx);
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
                return Ok(out);
            }
        };
    }
    cast!(Int8Array);
    cast!(Int16Array);
    cast!(Int32Array);
    cast!(Int64Array);
    cast!(UInt8Array);
    cast!(UInt16Array);
    cast!(UInt32Array);
    cast!(UInt64Array);
    cast!(Float32Array);
    cast!(Float64Array);
    Err(MiceNodeError::NonNumericColumn {
        name: name.to_string(),
        dtype: format!("{:?}", col.data_type()),
    })
}

/// Pull a boolean column (length n) marking observed/missing entries.
pub fn extract_observed(batches: &[RecordBatch], name: &str) -> Result<Vec<bool>, MiceNodeError> {
    // Treat NaN in f64 columns as missing by default; callers specify which
    // column is the response.
    let vals = extract_f64(batches, name)?;
    Ok(vals.into_iter().map(|v| !v.is_nan()).collect())
}

/// Build the predictor design matrix: each row is the predictor values at
/// row `i`. Returns `(x, n, p)` where `x` is row-major flat `n × p` and
/// `p = predictors.len()`.
pub fn build_predictor_matrix(
    batches: &[RecordBatch],
    predictors: &[String],
) -> Result<(Vec<Vec<f64>>, usize), MiceNodeError> {
    if predictors.is_empty() {
        // Return n × 0 design.
        let n = batches.iter().map(|b| b.num_rows()).sum();
        return Ok((Vec::new(), n));
    }
    let mut cols: Vec<Vec<f64>> = Vec::with_capacity(predictors.len());
    for p in predictors {
        cols.push(extract_f64(batches, p)?);
    }
    let n = cols[0].len();
    let mut rows: Vec<Vec<f64>> = Vec::with_capacity(n);
    for i in 0..n {
        let mut row = Vec::with_capacity(predictors.len());
        for c in &cols {
            row.push(c[i]);
        }
        rows.push(row);
    }
    Ok((rows, n))
}

/// Output schema for a single-column imputation result.
pub fn imputation_output_schema() -> SchemaRef {
    Arc::new(Schema::new(vec![arrow_schema::Field::new(
        "imputed",
        DataType::Float64,
        false,
    )]))
}

/// Build a single-column Float64 RecordBatch from a `Vec<f64>`.
pub fn build_imputation_batch(values: &[f64]) -> Result<RecordBatch, arrow_schema::ArrowError> {
    RecordBatch::try_new(
        imputation_output_schema(),
        vec![Arc::new(Float64Array::from(values.to_vec()))],
    )
}

/// Build a NodeCtx for unit tests.
pub fn test_node_ctx() -> NodeCtx {
    NodeCtx {
        runtime_env: datafusion::prelude::SessionContext::new().runtime_env(),
        iceberg_catalog: None,
        datalake: std::sync::Arc::new(datalake::Datalake::default()),
        opendal: None,
        resources: std::sync::Arc::new(dag_core::resource_catalog::ResourceCatalog::new(std::path::PathBuf::from("."))),
        global_sem: None,
    }
}
