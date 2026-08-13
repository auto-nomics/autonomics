//! DAG node: `grf_lm_forest`
//!
//! Trains a linear-model forest that estimates the K coefficients of a
//! conditional linear model
//!
//! ```text
//!   Y = c(x) + h_1(x) · W_1 + ... + h_K(x) · W_K
//! ```
//!
//! where Y may be multi-outcome. Mirrors grf R's `lm_forest`.

use std::sync::Arc;

use arrow_array::{Array, Float64Array, RecordBatch};
use arrow_schema::{DataType, Field, Schema, SchemaRef};
use schemars::{JsonSchema, schema_for};
use serde::{Deserialize, Serialize};

use crate::data::Matrix;
use crate::forest::{ForestBlob, ForestStats, LmSpec, LmTrainer, OobPredictions};
use crate::nodes::regression_forest::{
    NodeTrainOptions, arrow_batches_to_f64, arrow_batches_to_matrix,
};
use crate::{GrfError, Result};
use grf_sys as sys;

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct LmForestSpec {
    /// Names of X columns used as features.
    #[serde(default)]
    pub x_column_names: Vec<String>,
    /// Names of Y (response) columns. At least 1.
    pub y_column_names: Vec<String>,
    /// Names of W (regressor) columns. At least 1.
    pub w_column_names: Vec<String>,
    /// Per-coefficient weights for the split criterion Δ·gradient_weights.
    /// `None` → uniform.
    #[serde(default)]
    pub gradient_weights: Option<Vec<f64>>,
    /// Optional per-sample weights column.
    #[serde(default)]
    pub sample_weights_column: Option<String>,
    /// Forest hyperparameters.
    #[serde(default)]
    pub options: NodeTrainOptions,
}

#[derive(Debug, Clone)]
pub struct LmForestOutput {
    pub forest: ForestBlob,
    pub oob_predictions: Option<OobPredictions>,
    pub stats: ForestStats,
}

pub struct LmForestFactory;

impl LmForestFactory {
    pub fn kind() -> &'static str {
        "grf_lm_forest"
    }
}

impl LmForestSpec {
    pub fn fit(&self, batches: &[RecordBatch]) -> Result<LmForestOutput> {
        let n_rows: usize = batches.iter().map(|b| b.num_rows()).sum();
        if n_rows == 0 {
            return Err(GrfError::Missing("no rows".into()));
        }
        let schema = batches[0].schema();
        let mut reserved: Vec<&String> = self.y_column_names.iter().collect();
        reserved.extend(self.w_column_names.iter());
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
        let y_columns = collect_y_columns(batches, &self.y_column_names, n_rows)?;
        let w_columns = collect_y_columns(batches, &self.w_column_names, n_rows)?;
        let weights = self
            .sample_weights_column
            .as_ref()
            .map(|c| arrow_batches_to_f64(batches, c, n_rows))
            .transpose()?;

        let trained = LmTrainer::fit(LmSpec {
            x: x_matrix,
            y_columns,
            w_columns,
            sample_weights: weights,
            options: self.options.to_sys(),
        })?;
        let oob = trained.oob_predictions();
        let stats = ForestStats::from(&trained);
        Ok(LmForestOutput {
            forest: trained,
            oob_predictions: oob,
            stats,
        })
    }
}

fn collect_y_columns(
    batches: &[RecordBatch],
    cols: &[String],
    n_rows: usize,
) -> Result<Vec<Vec<f64>>> {
    cols.iter()
        .map(|name| arrow_batches_to_f64(batches, name, n_rows))
        .collect()
}

#[allow(dead_code)]
fn _schema() -> SchemaRef {
    Arc::new(Schema::new(vec![Field::new("x", DataType::Float64, false)]))
}

#[allow(dead_code)]
fn _schemars() -> schemars::Schema {
    schema_for!(LmForestSpec)
}

#[allow(dead_code)]
fn _sys_marker(_: sys::TrainOptions) {}
