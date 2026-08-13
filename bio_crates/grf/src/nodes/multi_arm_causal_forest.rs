//! DAG node: `grf_multi_arm_causal_forest`
//!
//! Trains a multi-arm / multi-outcome causal forest for K-arm
//! randomized designs with M outcome variables.

use std::sync::Arc;

use arrow_array::{Array, Float64Array, RecordBatch};
use arrow_schema::{DataType, Field, Schema, SchemaRef};
use schemars::{JsonSchema, schema_for};
use serde::{Deserialize, Serialize};

use crate::data::Matrix;
use crate::forest::{ForestBlob, ForestStats, MultiCausalSpec, MultiCausalTrainer, OobPredictions};
use crate::nodes::regression_forest::{
    NodeTrainOptions, arrow_batches_to_f64, arrow_batches_to_matrix,
};
use crate::{GrfError, Result};
use grf_sys as sys;

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct MultiArmCausalForestSpec {
    /// Names of X columns used as features.
    #[serde(default)]
    pub x_column_names: Vec<String>,
    /// Names of Y (outcome) columns. Multi-outcome supported.
    pub y_column_names: Vec<String>,
    /// Name of the treatment column (factor / integer 1..K).
    pub w_column_name: String,
    /// Per-coefficient weights for the split criterion.
    #[serde(default)]
    pub gradient_weights: Option<Vec<f64>>,
    /// Whether to use stabilization in the splits.
    #[serde(default = "default_true")]
    pub stabilize_splits: bool,
    /// Optional per-sample weights column.
    #[serde(default)]
    pub sample_weights_column: Option<String>,
    /// Forest hyperparameters.
    #[serde(default)]
    pub options: NodeTrainOptions,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone)]
pub struct MultiArmCausalForestOutput {
    pub forest: ForestBlob,
    pub oob_predictions: Option<OobPredictions>,
    pub stats: ForestStats,
}

pub struct MultiArmCausalForestFactory;

impl MultiArmCausalForestFactory {
    pub fn kind() -> &'static str {
        "grf_multi_arm_causal_forest"
    }
}

impl MultiArmCausalForestSpec {
    pub fn fit(&self, batches: &[RecordBatch]) -> Result<MultiArmCausalForestOutput> {
        let n_rows: usize = batches.iter().map(|b| b.num_rows()).sum();
        if n_rows == 0 {
            return Err(GrfError::Missing("no rows".into()));
        }
        let schema = batches[0].schema();
        let mut reserved: Vec<&String> = self.y_column_names.iter().collect();
        reserved.push(&self.w_column_name);
        let x_cols = if self.x_column_names.is_empty() {
            schema
                .fields()
                .iter()
                .filter(|f| {
                    !reserved.iter().any(|r| *r == f.name())
                        && matches!(f.data_type(), DataType::Float64)
                })
                .map(|f| f.name().clone())
                .collect()
        } else {
            self.x_column_names.clone()
        };
        let x_matrix = arrow_batches_to_matrix(batches, &x_cols, n_rows)?;
        let y_columns: Vec<Vec<f64>> = self
            .y_column_names
            .iter()
            .map(|name| arrow_batches_to_f64(batches, name, n_rows))
            .collect::<Result<Vec<_>>>()?;
        let w = arrow_batches_to_f64(batches, &self.w_column_name, n_rows)?;
        // Determine K from the number of distinct treatment levels.
        let num_treatments = {
            let mut levels: Vec<i64> = w.iter().map(|&v| v as i64).collect();
            levels.sort_unstable();
            levels.dedup();
            levels.len()
        };
        let weights = self
            .sample_weights_column
            .as_ref()
            .map(|c| arrow_batches_to_f64(batches, c, n_rows))
            .transpose()?;

        let trained = MultiCausalTrainer::fit(MultiCausalSpec {
            x: x_matrix,
            y_columns,
            w,
            num_treatments,
            gradient_weights: self.gradient_weights.clone(),
            stabilize_splits: self.stabilize_splits,
            sample_weights: weights,
            options: self.options.to_sys(),
        })?;
        let oob = trained.oob_predictions();
        let stats = ForestStats::from(&trained);
        Ok(MultiArmCausalForestOutput {
            forest: trained,
            oob_predictions: oob,
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
    schema_for!(MultiArmCausalForestSpec)
}

#[allow(dead_code)]
fn _sys_marker(_: sys::TrainOptions) {}
