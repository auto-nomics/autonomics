//! DAG node: `grf_ll_regression_forest`
//!
//! Trains a local-linear regression forest (Friedberg, Tibshirani, Athey 2020).
//! The splitting criterion is ridge residuals rather than standard CART.

use std::sync::Arc;

use arrow_array::{Array, Float64Array, RecordBatch};
use arrow_schema::{DataType, Field, Schema, SchemaRef};
use schemars::{JsonSchema, schema_for};
use serde::{Deserialize, Serialize};

use crate::data::Matrix;
use crate::forest::{ForestBlob, ForestStats, LlRegressionSpec, LlRegressionTrainer};
use crate::nodes::regression_forest::{
    NodeTrainOptions, arrow_batches_to_f64, arrow_batches_to_matrix,
};
use crate::{GrfError, Result};
use grf_sys as sys;

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct LlRegressionForestSpec {
    /// Names of X columns used as features.
    #[serde(default)]
    pub x_column_names: Vec<String>,
    /// Name of the outcome column.
    pub y_column_name: String,
    /// Optional per-sample weights column.
    #[serde(default)]
    pub sample_weights_column: Option<String>,
    /// Local-linear splitting parameters.
    #[serde(default)]
    pub ll_split_lambda: f64,
    #[serde(default)]
    pub ll_split_weight_penalty: bool,
    /// Subset of columns to use in the LL correction (empty = all).
    #[serde(default)]
    pub ll_split_variables: Vec<String>,
    /// Cutoff leaf size for switching from leaf betas to overall betas.
    /// Default 0 → grf's sqrt(n).
    #[serde(default)]
    pub ll_split_cutoff: usize,
    /// Forest hyperparameters.
    #[serde(default)]
    pub options: NodeTrainOptions,
}

#[derive(Debug, Clone)]
pub struct LlRegressionForestOutput {
    pub forest: ForestBlob,
    pub stats: ForestStats,
}

pub struct LlRegressionForestFactory;

impl LlRegressionForestFactory {
    pub fn kind() -> &'static str {
        "grf_ll_regression_forest"
    }
}

impl LlRegressionForestSpec {
    pub fn fit(&self, batches: &[RecordBatch]) -> Result<LlRegressionForestOutput> {
        let n_rows: usize = batches.iter().map(|b| b.num_rows()).sum();
        if n_rows == 0 {
            return Err(GrfError::Missing("no rows".into()));
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
        let y = arrow_batches_to_f64(batches, &self.y_column_name, n_rows)?;
        let weights = self
            .sample_weights_column
            .as_ref()
            .map(|c| arrow_batches_to_f64(batches, c, n_rows))
            .transpose()?;

        // Resolve ll_split_variables names to indices. Empty → all columns.
        let ll_split_variables: Vec<usize> = if self.ll_split_variables.is_empty() {
            (0..x_cols.len()).collect()
        } else {
            self.ll_split_variables
                .iter()
                .map(|name| {
                    x_cols.iter().position(|c| c == name).ok_or_else(|| {
                        GrfError::Shape(format!("ll_split_variables column '{}' not in X", name))
                    })
                })
                .collect::<Result<Vec<_>>>()?
        };
        // grf default: cutoff = sqrt(n).
        let cutoff = if self.ll_split_cutoff == 0 {
            (n_rows as f64).sqrt() as usize
        } else {
            self.ll_split_cutoff
        };

        let trained = LlRegressionTrainer::fit(LlRegressionSpec {
            x: x_matrix,
            y,
            ll_split_lambda: self.ll_split_lambda,
            ll_split_weight_penalty: self.ll_split_weight_penalty,
            ll_split_variables,
            ll_split_cutoff: cutoff,
            sample_weights: weights,
            options: self.options.to_sys(),
        })?;
        let stats = ForestStats::from(&trained);
        Ok(LlRegressionForestOutput {
            forest: trained,
            stats,
        })
    }
}

#[allow(dead_code)]
fn _schema() -> SchemaRef {
    Arc::new(Schema::new(vec![Field::new("x", DataType::Float64, false)]))
}

#[allow(dead_code)]
fn _schemars() -> schemars::Schema {
    schema_for!(LlRegressionForestSpec)
}

#[allow(dead_code)]
fn _sys_marker(_: sys::TrainOptions) {}
