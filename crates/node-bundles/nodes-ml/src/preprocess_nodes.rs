//! Preprocessing DAG nodes — scalers, encoders, imputers, transforms.
//!
//! Each node reads numeric columns from the input port, applies a transform,
//! and outputs the transformed table. Fitted scaler parameters are available
//! as an optional model-artifact output port when `emit_model = true`.

use std::sync::Arc;

use arrow_array::{Array, Float64Array, Int32Array, RecordBatch, StringArray, UInt32Array};
use arrow_schema::{DataType, Field, Schema};
use async_trait::async_trait;
use faer::Mat;
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;

use super::common;
use dag_core::dag::{DagError, graph::PortOutputs};
use dag_core::node::{DagNode, NodeInput, NodePorts};
use dag_core::registry::{NodeCtx, NodeFactory};

use ml::preprocess::{
    ImputeStrategy, Imputer, MinMaxScaler, NormKind, PowerTransformer, RobustScaler,
    StandardScaler, normalize_rows,
};

// ── helper: extract feature matrix from input ───────────────────────────

fn extract_features(batches: &[RecordBatch], columns: &[String]) -> Result<Mat<f64>, DagError> {
    common::extract_matrix(batches, columns).map_err(|e| DagError::NodeError {
        node_type: "ml_preprocess".into(),
        msg: e.to_string(),
    })
}

/// Replace the given columns in a batch with transformed values, preserving
/// all other columns.
fn replace_columns(
    batches: &[RecordBatch],
    replace: &[String],
    new_data: &Mat<f64>,
    extra_cols: &[(&str, Vec<f64>)],
) -> Result<RecordBatch, DagError> {
    let schema = batches.first().ok_or(DagError::NodeError {
        node_type: "ml_preprocess".into(),
        msg: "no input rows".into(),
    })?;
    let orig_schema = schema.schema();
    let n_rows = new_data.nrows();

    let mut fields: Vec<(Arc<Field>, Vec<Arc<dyn Array>>)> = Vec::new();

    // Re-emit original columns that are NOT in `replace`
    let replace_set: std::collections::HashSet<&str> = replace.iter().map(|s| s.as_str()).collect();

    for (col_i, field) in orig_schema.fields().iter().enumerate() {
        if replace_set.contains(field.name().as_str()) {
            continue;
        }
        let mut col_values: Vec<Arc<dyn Array>> = Vec::new();
        for batch in batches {
            col_values.push(batch.column(col_i).clone());
        }
        fields.push((field.clone(), col_values));
    }

    // Add transformed columns
    let (nrows, _ncols) = new_data.shape();
    let mut all_fields: Vec<Arc<Field>> = fields.iter().map(|(f, _)| f.clone()).collect();
    let mut all_arrays: Vec<Arc<dyn Array>> =
        fields.into_iter().flat_map(|(_, arrays)| arrays).collect();

    for (j, name) in replace.iter().enumerate() {
        let col_data: Vec<f64> = (0..nrows).map(|i| new_data[(i, j)]).collect();
        all_fields.push(Arc::new(Field::new(name, DataType::Float64, true)));
        all_arrays.push(Arc::new(Float64Array::from(col_data)));
    }

    for (name, values) in extra_cols {
        all_fields.push(Arc::new(Field::new(*name, DataType::Float64, true)));
        all_arrays.push(Arc::new(Float64Array::from(values.clone())));
    }

    let _ = n_rows;
    RecordBatch::try_new(Arc::new(Schema::new(all_fields)), all_arrays).map_err(|e| {
        DagError::NodeError {
            node_type: "ml_preprocess".into(),
            msg: format!("failed to build output batch: {e}"),
        }
    })
}

mod standardize;
pub use standardize::StandardizeFactory;

mod min_max_scale;
pub use min_max_scale::MinMaxScaleFactory;

mod robust_scale;
pub use robust_scale::RobustScaleFactory;

mod normalize_rows;
pub use normalize_rows::NormalizeRowsFactory;

mod power_transform;
pub use power_transform::PowerTransformFactory;

mod impute;
pub use impute::ImputeFactory;

mod one_hot_encode;
pub use one_hot_encode::OneHotEncodeFactory;

mod label_encode;
pub use label_encode::LabelEncodeFactory;

mod poly_features;
pub use poly_features::PolyFeaturesFactory;

mod k_bins_discretize;
pub use k_bins_discretize::KBinsDiscretizeFactory;

// ═══════════════════════════════════════════════════════════════════════
// Shared helpers
// ═══════════════════════════════════════════════════════════════════════

async fn collect_batches(inputs: &[NodeInput]) -> Result<Vec<RecordBatch>, DagError> {
    let input = inputs.first().ok_or(DagError::NodeError {
        node_type: "ml".into(),
        msg: "no input port connected".into(),
    })?;
    input
        .dataframe()?
        .clone()
        .collect()
        .await
        .map_err(|e| DagError::NodeError {
            node_type: "ml".into(),
            msg: format!("collect failed: {e}"),
        })
}

fn emit_batch(ctx: &NodeCtx, batch: RecordBatch) -> Result<PortOutputs, DagError> {
    let df = ctx
        .session()
        .read_batch(batch)
        .map_err(|e| DagError::NodeError {
            node_type: "ml".into(),
            msg: format!("read_batch failed: {e}"),
        })?;
    let mut res = PortOutputs::new();
    res.insert(0, df);
    Ok(res)
}

fn replace_columns_with_names(
    batches: &[RecordBatch],
    new_col_names: &[String],
    new_data: &Mat<f64>,
) -> Result<RecordBatch, DagError> {
    let (_schema, mut fields, mut arrays) = common::concat_input(batches)?;
    let existing_names: std::collections::HashSet<String> = fields
        .iter()
        .map(|field| field.name().to_string())
        .collect();
    let mut generated_names = std::collections::HashSet::new();

    let (nrows, ncols) = new_data.shape();
    for j in 0..ncols {
        let name = new_col_names
            .get(j)
            .map(String::as_str)
            .ok_or_else(|| DagError::NodeError {
                node_type: "ml_poly_features".into(),
                msg: format!("missing name for generated feature {j}"),
            })?;
        if existing_names.contains(name) || !generated_names.insert(name.to_string()) {
            return Err(DagError::NodeError {
                node_type: "ml_poly_features".into(),
                msg: format!("generated feature name '{name}' collides with another output column"),
            });
        }

        let col_data: Vec<f64> = (0..nrows).map(|i| new_data[(i, j)]).collect();
        fields.push(Arc::new(Field::new(name, DataType::Float64, true)));
        arrays.push(Arc::new(Float64Array::from(col_data)));
    }

    RecordBatch::try_new(Arc::new(Schema::new(fields)), arrays).map_err(|e| DagError::NodeError {
        node_type: "ml_poly_features".into(),
        msg: format!("failed to build output: {e}"),
    })
}
