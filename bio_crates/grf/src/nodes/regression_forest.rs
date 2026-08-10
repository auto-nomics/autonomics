//! DAG node: `grf_regression_forest`
//!
//! Trains a regression forest (`grf::regression_forest` in R) and emits the
//! trained forest blob plus OOB predictions on the training set.

use std::sync::Arc;

use arrow_array::{Array, Float64Array, RecordBatch};
use arrow_schema::{DataType, Field, Schema, SchemaRef};
use schemars::{schema_for, JsonSchema};
use serde::{Deserialize, Serialize};

use crate::data::Matrix;
use crate::forest::{ForestBlob, ForestStats, OobPredictions, RegressionSpec, RegressionTrainer};
use crate::{GrfError, Result};
use grf_sys as sys;

/// DAG spec for the regression-forest trainer. The user-facing spec is a
/// flat struct (so it serializes cleanly as JSON/YAML in DAG definitions).
/// `x_column_names`, `y_column_name`, and `sample_weights_column` reference
/// columns in the input RecordBatch(es).
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct RegressionForestSpec {
    /// Names of columns in `port 0` (the X RecordBatch) that should be used
    /// as features. All must be numeric (`Float64`). If empty, every
    /// numeric column except `y_column_name` is used.
    #[serde(default)]
    pub x_column_names: Vec<String>,
    /// Name of the column holding the outcome Y. Must be numeric.
    pub y_column_name: String,
    /// Optional column of per-sample weights.
    #[serde(default)]
    pub sample_weights_column: Option<String>,
    /// Forest hyperparameters. Defaults match grf::ForestOptions defaults.
    #[serde(default)]
    pub options: NodeTrainOptions,
}

/// Mirror of `sys::TrainOptions` but serde-friendly.
///
/// IMPORTANT: `Default::default()` MUST match `#[serde(default = ...)]`
/// defaults above. Using `..Default::default()` in DAG specs only works
/// if both surfaces agree; otherwise grf sees zeros and divides by zero.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct NodeTrainOptions {
    #[serde(default = "default_num_trees")]
    pub num_trees: u32,
    #[serde(default = "default_ci_group_size")]
    pub ci_group_size: u32,
    #[serde(default = "default_sample_fraction")]
    pub sample_fraction: f64,
    #[serde(default)]
    pub mtry: u32,
    #[serde(default = "default_min_node_size")]
    pub min_node_size: u32,
    #[serde(default = "default_true")]
    pub honesty: bool,
    #[serde(default = "default_honesty_fraction")]
    pub honesty_fraction: f64,
    #[serde(default = "default_true")]
    pub honesty_prune_leaves: bool,
    #[serde(default = "default_alpha")]
    pub alpha: f64,
    #[serde(default)]
    pub imbalance_penalty: f64,
    #[serde(default)]
    pub num_threads: u32,
    #[serde(default = "default_seed")]
    pub seed: u32,
    #[serde(default)]
    pub legacy_seed: bool,
    #[serde(default = "default_true")]
    pub compute_oob_predictions: bool,
}

fn default_num_trees() -> u32 { 2000 }
fn default_ci_group_size() -> u32 { 2 }
fn default_sample_fraction() -> f64 { 0.5 }
fn default_min_node_size() -> u32 { 5 }
fn default_true() -> bool { true }
fn default_honesty_fraction() -> f64 { 0.5 }
fn default_alpha() -> f64 { 0.05 }
fn default_seed() -> u32 { 42 }

impl Default for NodeTrainOptions {
    fn default() -> Self {
        Self {
            num_trees: default_num_trees(),
            ci_group_size: default_ci_group_size(),
            sample_fraction: default_sample_fraction(),
            mtry: 0,
            min_node_size: default_min_node_size(),
            honesty: default_true(),
            honesty_fraction: default_honesty_fraction(),
            honesty_prune_leaves: default_true(),
            alpha: default_alpha(),
            imbalance_penalty: 0.0,
            num_threads: 0,
            seed: default_seed(),
            legacy_seed: false,
            compute_oob_predictions: default_true(),
        }
    }
}

impl NodeTrainOptions {
    pub fn to_sys(&self) -> sys::TrainOptions {
        let mut opts = sys::TrainOptions::default();
        opts.num_trees = self.num_trees;
        opts.ci_group_size = self.ci_group_size;
        opts.sample_fraction = self.sample_fraction;
        opts.mtry = self.mtry;
        opts.min_node_size = self.min_node_size;
        opts.honesty = self.honesty;
        opts.honesty_fraction = self.honesty_fraction;
        opts.honesty_prune_leaves = self.honesty_prune_leaves;
        opts.alpha = self.alpha;
        opts.imbalance_penalty = self.imbalance_penalty;
        opts.num_threads = self.num_threads;
        opts.seed = self.seed;
        // legacy_seed = true matches grf R's default behaviour
        // (legacy.seed = !is.null(seed)).
        opts.legacy_seed = true;
        opts.compute_oob_predictions = self.compute_oob_predictions;
        opts
    }
}

#[derive(Debug, Clone)]
pub struct RegressionForestOutput {
    pub forest: ForestBlob,
    pub oob_predictions: Option<OobPredictions>,
    pub stats: ForestStats,
}

pub struct RegressionForestFactory;

impl RegressionForestFactory {
    pub fn kind() -> &'static str { "grf_regression_forest" }
}

impl RegressionForestSpec {
    /// Train a forest from collected RecordBatches. The caller (the DAG node)
    /// is responsible for collecting from any async DataFusion source.
    pub fn fit(&self, batches: &[RecordBatch]) -> Result<RegressionForestOutput> {
        let n_rows: usize = batches.iter().map(|b| b.num_rows()).sum();
        if n_rows == 0 {
            return Err(GrfError::Missing("no rows in input batches".into()));
        }
        let schema = batches[0].schema();
        let x_cols = if self.x_column_names.is_empty() {
            schema.fields().iter()
                .filter(|f| f.name() != &self.y_column_name
                    && matches!(f.data_type(), DataType::Float64))
                .map(|f| f.name().clone()).collect()
        } else {
            self.x_column_names.clone()
        };
        let x_matrix = arrow_batches_to_matrix(batches, &x_cols, n_rows)?;
        let y_vec = arrow_batches_to_f64(batches, &self.y_column_name, n_rows)?;
        let weights = self.sample_weights_column.as_ref()
            .map(|c| arrow_batches_to_f64(batches, c, n_rows))
            .transpose()?;

        let trained = RegressionTrainer::fit(RegressionSpec {
            x: x_matrix, y: y_vec, sample_weights: weights,
            options: self.options.to_sys(),
        })?;
        let oob = trained.oob_predictions();
        let stats = ForestStats::from(&trained);
        Ok(RegressionForestOutput { forest: trained, oob_predictions: oob, stats })
    }
}

// ─────────────────────────── helpers ───────────────────────────

/// Pull `n_rows` values from each named column of `batches` into a
/// column-major `Matrix`. Assumes columns are Float64.
pub(crate) fn arrow_batches_to_matrix(
    batches: &[RecordBatch], cols: &[String], n_rows: usize,
) -> Result<Matrix> {
    use arrow_array::Int64Array;
    let mut buf = vec![0f64; n_rows * cols.len()];
    let mut offset = 0;
    for batch in batches {
        let r = batch.num_rows();
        for (j, name) in cols.iter().enumerate() {
            let arr = batch.column_by_name(name)
                .ok_or_else(|| GrfError::Shape(format!("column '{}' not found", name)))?;
            // Accept Float64 or Int64 (convert to f64).
            if let Some(f) = arr.as_any().downcast_ref::<Float64Array>() {
                for i in 0..r {
                    buf[j * n_rows + offset + i] = if f.is_null(i) { f64::NAN } else { f.value(i) };
                }
            } else if let Some(i64arr) = arr.as_any().downcast_ref::<Int64Array>() {
                for i in 0..r {
                    buf[j * n_rows + offset + i] = if i64arr.is_null(i) { f64::NAN } else { i64arr.value(i) as f64 };
                }
            } else {
                return Err(GrfError::Shape(format!(
                    "column '{}' is neither Float64 nor Int64", name
                )));
            }
        }
        offset += r;
    }
    Ok(Matrix { data: buf, n_rows, n_cols: cols.len() })
}

pub(crate) fn arrow_batches_to_f64(
    batches: &[RecordBatch], col: &str, n_rows: usize,
) -> Result<Vec<f64>> {
    let mut out = Vec::with_capacity(n_rows);
    for batch in batches {
        let arr = batch.column_by_name(col)
            .ok_or_else(|| GrfError::Shape(format!("column '{}' not found", col)))?;
        let arr = arr.as_any().downcast_ref::<Float64Array>()
            .ok_or_else(|| GrfError::Shape(format!("column '{}' is not Float64", col)))?;
        for i in 0..batch.num_rows() {
            out.push(if arr.is_null(i) { f64::NAN } else { arr.value(i) });
        }
    }
    if out.len() != n_rows {
        return Err(GrfError::Shape(format!(
            "column '{}' has {} rows, expected {}", col, out.len(), n_rows
        )));
    }
    Ok(out)
}

#[allow(dead_code)]
fn _schema() -> SchemaRef {
    Arc::new(Schema::new(vec![Field::new("x", DataType::Float64, false)]))
}

#[allow(dead_code)]
fn _schemars() -> schemars::Schema { schema_for!(RegressionForestSpec) }