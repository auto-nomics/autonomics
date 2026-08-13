//! DAG node: `grf_probability_forest`
//!
//! Trains a probability (multiclass classification) forest.

use std::sync::Arc;

use arrow_array::{Array, Float64Array, Int64Array, RecordBatch};
use arrow_schema::{DataType, Field, Schema, SchemaRef};
use schemars::{JsonSchema, schema_for};
use serde::{Deserialize, Serialize};

use crate::data::Matrix;
use crate::forest::{ForestBlob, ForestStats, OobPredictions, ProbabilitySpec, ProbabilityTrainer};
use crate::nodes::regression_forest::{NodeTrainOptions, arrow_batches_to_matrix};
use crate::{GrfError, Result};
use grf_sys as sys;

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct ProbabilityForestSpec {
    /// Names of columns in `port 0` (the X RecordBatch) used as features.
    #[serde(default)]
    pub x_column_names: Vec<String>,
    /// Name of the column holding the class label (Int64 with values in
    /// 0..num_classes).
    pub y_column_name: String,
    /// Number of distinct class labels.
    pub num_classes: usize,
    /// Optional column of per-sample weights.
    #[serde(default)]
    pub sample_weights_column: Option<String>,
    /// Forest hyperparameters.
    #[serde(default)]
    pub options: NodeTrainOptions,
}

#[derive(Debug, Clone)]
pub struct ProbabilityForestOutput {
    pub forest: ForestBlob,
    /// Per-class probabilities per OOB row. Layout: column-major,
    /// `num_classes × n_rows` (since grf's `Prediction.size()` equals
    /// `num_classes`).
    pub oob_predictions: Option<OobPredictions>,
    pub stats: ForestStats,
}

pub struct ProbabilityForestFactory;

impl ProbabilityForestFactory {
    pub fn kind() -> &'static str {
        "grf_probability_forest"
    }
}

impl ProbabilityForestSpec {
    pub fn fit(&self, batches: &[RecordBatch]) -> Result<ProbabilityForestOutput> {
        let n_rows: usize = batches.iter().map(|b| b.num_rows()).sum();
        if n_rows == 0 {
            return Err(GrfError::Missing("no rows in input batches".into()));
        }
        let schema = batches[0].schema();
        let x_cols = if self.x_column_names.is_empty() {
            schema
                .fields()
                .iter()
                .filter(|f| {
                    f.name() != &self.y_column_name && matches!(f.data_type(), DataType::Float64)
                })
                .map(|f| f.name().clone())
                .collect()
        } else {
            self.x_column_names.clone()
        };
        let x_matrix = arrow_batches_to_matrix(batches, &x_cols, n_rows)?;
        let y_vec = arrow_batches_to_int64(batches, &self.y_column_name, n_rows)?;

        // Validate class labels are in [0, num_classes).
        if let Some(&bad) = y_vec
            .iter()
            .find(|&&c| c < 0 || c >= self.num_classes as i64)
        {
            return Err(GrfError::Shape(format!(
                "class label {} outside [0, {})",
                bad, self.num_classes
            )));
        }

        let weights = self
            .sample_weights_column
            .as_ref()
            .map(|c| arrow_batches_to_f64_safe(batches, c, n_rows))
            .transpose()?;

        let trained = ProbabilityTrainer::fit(ProbabilitySpec {
            x: x_matrix,
            y: y_vec.into_iter().map(|c| c as f64).collect(),
            num_classes: self.num_classes,
            sample_weights: weights,
            options: self.options.to_sys(),
        })?;

        let oob = trained.oob_predictions();
        let stats = ForestStats::from(&trained);
        Ok(ProbabilityForestOutput {
            forest: trained,
            oob_predictions: oob,
            stats,
        })
    }
}

fn arrow_batches_to_int64(batches: &[RecordBatch], col: &str, n_rows: usize) -> Result<Vec<i64>> {
    let mut out = Vec::with_capacity(n_rows);
    for batch in batches {
        let arr = batch
            .column_by_name(col)
            .ok_or_else(|| GrfError::Shape(format!("column '{}' not found", col)))?;
        let arr = arr
            .as_any()
            .downcast_ref::<Int64Array>()
            .ok_or_else(|| GrfError::Shape(format!("column '{}' is not Int64", col)))?;
        for i in 0..batch.num_rows() {
            out.push(if arr.is_null(i) { 0 } else { arr.value(i) });
        }
    }
    Ok(out)
}

fn arrow_batches_to_f64_safe(
    batches: &[RecordBatch],
    col: &str,
    n_rows: usize,
) -> Result<Vec<f64>> {
    let mut out = Vec::with_capacity(n_rows);
    for batch in batches {
        let arr = batch
            .column_by_name(col)
            .ok_or_else(|| GrfError::Shape(format!("column '{}' not found", col)))?;
        let arr = arr
            .as_any()
            .downcast_ref::<Float64Array>()
            .ok_or_else(|| GrfError::Shape(format!("column '{}' is not Float64", col)))?;
        for i in 0..batch.num_rows() {
            out.push(if arr.is_null(i) {
                f64::NAN
            } else {
                arr.value(i)
            });
        }
    }
    Ok(out)
}

#[allow(dead_code)]
fn _schema() -> SchemaRef {
    Arc::new(Schema::new(vec![Field::new("x", DataType::Float64, false)]))
}

#[allow(dead_code)]
fn _schemars() -> schemars::Schema {
    schema_for!(ProbabilityForestSpec)
}

#[allow(dead_code)]
fn _sys_marker(_: sys::TrainOptions) {}
