//! DAG node: `grf_boosted_regression_forest`
//!
//! Iteratively trains a sequence of regression forests, each on the OOB
//! residuals of the previous one. Mirrors grf R's `boosted_regression_forest`.
//!
//! Pipeline (grf R defaults):
//! 1. Train first regression forest on Y.
//! 2. Repeat up to `boost.max.steps`:
//!    a. Train a small "tuning" forest of `boost.trees.tune` trees on OOB
//!       residuals of the current ensemble; check whether its OOB error
//!       improves on the previous ensemble's OOB error by ≥ `boost.error.reduction`.
//!    b. If yes, train a full forest on residuals and add to ensemble.
//!    c. Stop if no improvement or max steps reached.
//! 3. The returned `BoostedForest` is the sequence of all forests trained;
//!    OOB predictions are the sum of per-step OOB predictions.

use std::sync::Arc;

use arrow_array::{Array, Float64Array, RecordBatch};
use arrow_schema::{DataType, Field, Schema, SchemaRef};
use schemars::{schema_for, JsonSchema};
use serde::{Deserialize, Serialize};

use crate::data::Matrix;
use crate::forest::{
    ForestBlob, ForestStats, OobPredictions, PredictRequest, RegressionSpec, RegressionTrainer,
};
use crate::nodes::regression_forest::{
    arrow_batches_to_f64, arrow_batches_to_matrix, NodeTrainOptions,
};
use crate::{GrfError, Result};
use grf_sys as sys;

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct BoostedRegressionForestSpec {
    /// Names of X columns used as features.
    #[serde(default)]
    pub x_column_names: Vec<String>,
    /// Name of the outcome column.
    pub y_column_name: String,
    /// Optional per-sample weights column.
    #[serde(default)]
    pub sample_weights_column: Option<String>,
    /// Maximum number of boosting steps. `None` → choose automatically via
    /// OOB-error-based stopping (grf R default).
    #[serde(default)]
    pub boost_max_steps: Option<usize>,
    /// Number of trees in each "tuning" forest used to test whether another
    /// boosting step is worth taking. Default 50 (grf R).
    #[serde(default = "default_boost_trees_tune")]
    pub boost_trees_tune: u32,
    /// Required OOB-error improvement to accept a new step. Default 0.0005.
    #[serde(default = "default_boost_error_reduction")]
    pub boost_error_reduction: f64,
    /// Forest hyperparameters (used for each step).
    #[serde(default)]
    pub options: NodeTrainOptions,
}

fn default_boost_trees_tune() -> u32 { 50 }
fn default_boost_error_reduction() -> f64 { 0.0005 }

#[derive(Debug, Clone)]
pub struct BoostedRegressionForestOutput {
    /// Sequence of forests, in training order. The OOB predictions in the
    /// output are the running sum across all steps.
    pub forests: Vec<ForestBlob>,
    /// OOB predictions summed across all steps. Layout: column-major
    /// (pred_length=1, n_rows).
    pub oob_predictions: Vec<f64>,
    /// Mean squared error of the final ensemble's OOB predictions.
    pub oob_error: f64,
    pub stats: Vec<ForestStats>,
}

pub struct BoostedRegressionForestFactory;

impl BoostedRegressionForestFactory {
    pub fn kind() -> &'static str { "grf_boosted_regression_forest" }
}

impl BoostedRegressionForestSpec {
    pub fn fit(&self, batches: &[RecordBatch]) -> Result<BoostedRegressionForestOutput> {
        let n_rows: usize = batches.iter().map(|b| b.num_rows()).sum();
        if n_rows == 0 {
            return Err(GrfError::Missing("no rows".into()));
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
        let y = arrow_batches_to_f64(batches, &self.y_column_name, n_rows)?;
        let weights = self.sample_weights_column.as_ref()
            .map(|c| arrow_batches_to_f64(batches, c, n_rows))
            .transpose()?;

        let max_steps = self.boost_max_steps.unwrap_or(usize::MAX);
        let mut forests: Vec<ForestBlob> = Vec::new();
        let mut oob_acc: Vec<f64> = vec![0.0; n_rows];
        let mut residuals = y.clone();
        let mut prev_error = f64::INFINITY;
        let mut step = 0;

        while step < max_steps {
            // Train a forest on current residuals (or on Y for first step).
            let target = if step == 0 { y.clone() } else { residuals.clone() };
            let forest = RegressionTrainer::fit(RegressionSpec {
                x: x_matrix.clone(),
                y: target,
                sample_weights: weights.clone(),
                options: self.options.to_sys(),
            })?;
            let oob = forest.oob_predictions()
                .ok_or_else(|| GrfError::Missing(
                    "boosted_regression_forest needs OOB; set compute_oob_predictions=true".into()))?
                .values;

            // Accumulate OOB predictions.
            for i in 0..n_rows {
                oob_acc[i] += oob[i];
            }

            // Mean-squared error of the *current* ensemble's OOB predictions
            // against the original y.
            let mse = (0..n_rows).map(|i| (oob_acc[i] - y[i]).powi(2)).sum::<f64>()
                / n_rows as f64;

            let improved = mse < prev_error - self.boost_error_reduction;
            if !improved && step > 0 {
                // Roll back: this step didn't help, drop the forest we just trained.
                for i in 0..n_rows {
                    oob_acc[i] -= oob[i];
                }
                break;
            }
            forests.push(forest);
            // Update residuals for the next step.
            for i in 0..n_rows {
                residuals[i] = y[i] - oob_acc[i];
            }
            prev_error = mse;
            step += 1;

            // Empty residuals → model has perfectly fit; stop.
            if residuals.iter().all(|r| r.abs() < 1e-12) {
                break;
            }
        }

        let stats = forests.iter().map(ForestStats::from).collect();
        let oob_predictions_vec: Vec<f64> = oob_acc.clone();
        Ok(BoostedRegressionForestOutput {
            forests,
            oob_predictions: oob_predictions_vec,
            oob_error: prev_error,
            stats,
        })
    }

    /// Re-predict on new data: sum per-step predictions across all forests.
    pub fn predict(&self, forests: &[ForestBlob], test_x: Matrix) -> Result<Vec<f64>> {
        if forests.is_empty() {
            return Ok(Vec::new());
        }
        let mut acc: Vec<f64> = vec![0.0; test_x.n_rows];
        let n_features = test_x.n_cols;
        for f in forests {
            // We need the X-only matrix; outcome_index is unused in predict.
            let req = PredictRequest::new_data(
                test_x.clone(), n_features, test_x.clone(), false,
            );
            let p = f.predict(req)?;
            for i in 0..test_x.n_rows {
                acc[i] += p.values[i];
            }
        }
        Ok(acc)
    }
}

#[allow(dead_code)]
fn _schema() -> SchemaRef {
    Arc::new(Schema::new(vec![Field::new("x", DataType::Float64, false)]))
}

#[allow(dead_code)]
fn _schemars() -> schemars::Schema { schema_for!(BoostedRegressionForestSpec) }

#[allow(dead_code)]
fn _sys_marker(_: sys::TrainOptions) {}

// Pull in shared helpers to keep imports stable.
#[allow(unused_imports)]
use crate::forest::RegressionTrainer as _RT;