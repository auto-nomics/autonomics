//! DAG node: `grf_multi_regression_forest`
//!
//! Trains a multi-task regression forest for predicting multiple outcomes
//! jointly.

use std::sync::Arc;

use arrow_array::{Array, Float64Array, RecordBatch};
use arrow_schema::{DataType, Field, Schema, SchemaRef};
use schemars::{schema_for, JsonSchema};
use serde::{Deserialize, Serialize};

use crate::data::Matrix;
use crate::forest::{ForestBlob, ForestStats, MultiRegressionSpec, MultiRegressionTrainer, OobPredictions};
use crate::nodes::regression_forest::{
    arrow_batches_to_f64, arrow_batches_to_matrix, NodeTrainOptions,
};
use crate::{GrfError, Result};
use grf_sys as sys;

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct MultiRegressionForestSpec {
    /// Names of columns in `port 0` (the X RecordBatch) used as features.
    #[serde(default)]
    pub x_column_names: Vec<String>,
    /// Names of the outcome columns (one per task). At least 2.
    pub y_column_names: Vec<String>,
    /// Optional column of per-sample weights.
    #[serde(default)]
    pub sample_weights_column: Option<String>,
    /// Forest hyperparameters.
    #[serde(default)]
    pub options: NodeTrainOptions,
}

#[derive(Debug, Clone)]
pub struct MultiRegressionForestOutput {
    pub forest: ForestBlob,
    /// OOB predictions per task per sample. Layout: column-major
    /// (`num_tasks × n_rows`).
    pub oob_predictions: Option<OobPredictions>,
    pub stats: ForestStats,
}

pub struct MultiRegressionForestFactory;

impl MultiRegressionForestFactory {
    pub fn kind() -> &'static str { "grf_multi_regression_forest" }
}

impl MultiRegressionForestSpec {
    pub fn fit(&self, batches: &[RecordBatch]) -> Result<MultiRegressionForestOutput> {
        let n_rows: usize = batches.iter().map(|b| b.num_rows()).sum();
        if n_rows == 0 {
            return Err(GrfError::Missing("no rows in input batches".into()));
        }
        if self.y_column_names.len() < 2 {
            return Err(GrfError::Shape(format!(
                "multi_regression needs >= 2 outcome columns, got {}", self.y_column_names.len()
            )));
        }
        let schema = batches[0].schema();
        let x_cols = if self.x_column_names.is_empty() {
            schema.fields().iter()
                .filter(|f| !self.y_column_names.contains(f.name())
                    && matches!(f.data_type(), DataType::Float64))
                .map(|f| f.name().clone()).collect()
        } else {
            self.x_column_names.clone()
        };
        let x_matrix = arrow_batches_to_matrix(batches, &x_cols, n_rows)?;
        let y_columns: Vec<Vec<f64>> = self.y_column_names.iter()
            .map(|name| arrow_batches_to_f64(batches, name, n_rows))
            .collect::<Result<Vec<_>>>()?;
        let weights = self.sample_weights_column.as_ref()
            .map(|c| arrow_batches_to_f64(batches, c, n_rows))
            .transpose()?;

        let trained = MultiRegressionTrainer::fit(MultiRegressionSpec {
            x: x_matrix,
            y_columns,
            sample_weights: weights,
            options: self.options.to_sys(),
        })?;

        let oob = trained.oob_predictions();
        let stats = ForestStats::from(&trained);
        Ok(MultiRegressionForestOutput {
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
fn _schemars() -> schemars::Schema { schema_for!(MultiRegressionForestSpec) }

#[allow(dead_code)]
fn _sys_marker(_: sys::TrainOptions) {}

// Pull in the shared F64 helper to avoid an unused import warning in this
// module (arrow_batches_to_f64 is re-exported by regression_forest::NodeTrainOptions).
#[allow(dead_code)]
fn _f64_helper_marker(_: Vec<f64>) {}