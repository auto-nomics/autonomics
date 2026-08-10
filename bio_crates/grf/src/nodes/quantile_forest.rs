//! DAG node: `grf_quantile_forest`
//!
//! Trains a quantile forest (`grf::quantile_forest` in R) and emits the
//! trained forest blob plus per-quantile OOB predictions.

use std::sync::Arc;

use arrow_array::{Array, Float64Array, RecordBatch};
use arrow_schema::{DataType, Field, Schema, SchemaRef};
use schemars::{schema_for, JsonSchema};
use serde::{Deserialize, Serialize};

use crate::data::Matrix;
use crate::forest::{ForestBlob, ForestStats, QuantileSpec, QuantileTrainer};
use crate::nodes::regression_forest::{
    arrow_batches_to_f64, arrow_batches_to_matrix, NodeTrainOptions,
};
use crate::{GrfError, Result};
use grf_sys as sys;

/// DAG spec for the quantile-forest trainer.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct QuantileForestSpec {
    /// Names of columns in `port 0` (the X RecordBatch) used as features.
    /// If empty, every Float64 column except `y_column_name` is used.
    #[serde(default)]
    pub x_column_names: Vec<String>,
    /// Name of the column holding the outcome Y.
    pub y_column_name: String,
    /// Quantiles to estimate. Default mirrors grf R: (0.1, 0.5, 0.9).
    #[serde(default = "default_quantiles")]
    pub quantiles: Vec<f64>,
    /// If true, use regression splitting instead of quantile-aware splitting
    /// (Meinshausen 2006 style). Default false.
    #[serde(default)]
    pub regression_splitting: bool,
    /// Forest hyperparameters.
    #[serde(default)]
    pub options: NodeTrainOptions,
}

fn default_quantiles() -> Vec<f64> { vec![0.1, 0.5, 0.9] }

#[derive(Debug, Clone)]
pub struct QuantileForestOutput {
    pub forest: ForestBlob,
    /// Quantile values reported per OOB row. Layout: `Vec<f64>` of length
    /// `n_quantiles * n_rows` (column-major, quantile-major).
    pub oob_predictions: Option<Vec<f64>>,
    pub stats: ForestStats,
}

pub struct QuantileForestFactory;

impl QuantileForestFactory {
    pub fn kind() -> &'static str { "grf_quantile_forest" }
}

impl QuantileForestSpec {
    pub fn fit(&self, batches: &[RecordBatch]) -> Result<QuantileForestOutput> {
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

        // Override compute_oob_predictions in opts — quantile trainer doesn't
        // expose OOB via grf core (no prediction_strategy in quantile_trainer).
        let mut opts = self.options.to_sys();
        opts.compute_oob_predictions = false;

        let trained = QuantileTrainer::fit(QuantileSpec {
            x: x_matrix,
            y: y_vec,
            quantiles: self.quantiles.clone(),
            regression_splitting: self.regression_splitting,
            options: opts,
        })?;

        let stats = ForestStats::from(&trained);
        Ok(QuantileForestOutput {
            forest: trained,
            oob_predictions: None,  // grf core: quantile_trainer has no predict strategy
            stats,
        })
    }
}

#[allow(dead_code)]
fn _schema() -> SchemaRef {
    Arc::new(Schema::new(vec![Field::new("x", DataType::Float64, false)]))
}

#[allow(dead_code)]
fn _schemars() -> schemars::Schema { schema_for!(QuantileForestSpec) }

#[allow(dead_code)]
fn _sys_marker(_: sys::TrainOptions) {}