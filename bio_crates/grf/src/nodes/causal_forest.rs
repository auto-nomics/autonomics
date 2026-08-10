//! DAG node: `grf_causal_forest`
//!
//! Trains a causal forest for heterogeneous treatment effect estimation.
//! Implements the R-learner two-stage procedure internally:
//!
//!   1. Train `regression_forest(X, Y)` → Ŷ(X)
//!   2. Train `regression_forest(X, W)` → Ŵ(X)
//!   3. Compute residuals Y_centered = Y - Ŷ, W_centered = W - Ŵ
//!   4. Train `causal_forest` on (X, Y_centered, W_centered)
//!
//! Caller can also pass pre-computed `Y.hat` / `W.hat` (e.g. from a known
//! propensity model) to skip the first stage.

use std::sync::Arc;

use arrow_array::{Array, Float64Array, RecordBatch};
use arrow_schema::{DataType, Field, Schema, SchemaRef};
use schemars::{schema_for, JsonSchema};
use serde::{Deserialize, Serialize};

use crate::data::Matrix;
use crate::forest::{
    CausalSpec, CausalTrainer, ForestBlob, ForestStats, RegressionSpec, RegressionTrainer,
};
use crate::nodes::regression_forest::{
    arrow_batches_to_f64, arrow_batches_to_matrix, NodeTrainOptions,
};
use crate::{GrfError, Result};
use grf_sys as sys;

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct CausalForestSpec {
    /// Names of columns in `port 0` used as features.
    #[serde(default)]
    pub x_column_names: Vec<String>,
    /// Name of the outcome column (must be numeric).
    pub y_column_name: String,
    /// Name of the treatment column (binary or continuous).
    pub w_column_name: String,
    /// Pre-computed Ŷ(X); if omitted, trained internally via regression forest.
    #[serde(default)]
    pub y_hat: Option<Vec<f64>>,
    /// Pre-computed Ŵ(X); if omitted, trained internally via regression forest.
    #[serde(default)]
    pub w_hat: Option<Vec<f64>>,
    /// Whether to take treatment assignment into account when determining the
    /// imbalance of a split. Default true (matches grf R).
    #[serde(default = "default_true")]
    pub stabilize_splits: bool,
    /// Optional column of per-sample weights.
    #[serde(default)]
    pub sample_weights_column: Option<String>,
    /// Forest hyperparameters (also reused for the Y/W nuisance forests).
    #[serde(default)]
    pub options: NodeTrainOptions,
}

fn default_true() -> bool { true }

#[derive(Debug, Clone)]
pub struct CausalForestOutput {
    pub forest: ForestBlob,
    /// Ŷ(X) used by the R-learner. If the caller passed `y_hat`, equal to
    /// that input; otherwise the OOB predictions of the internal nuisance forest.
    pub y_hat: Vec<f64>,
    /// Ŵ(X) used by the R-learner.
    pub w_hat: Vec<f64>,
    /// OOB τ̂(X) (tau pointwise). Equal to `forest.oob_predictions()`.
    pub oob_predictions: Option<crate::forest::OobPredictions>,
    pub stats: ForestStats,
    /// Optional raw Y / W vectors captured at training time, used by
    /// downstream analysis nodes (ATE, BLP, RATE). The DAG node layer
    /// populates this automatically from the input RecordBatch.
    y_orig: Option<Vec<f64>>,
    w_orig: Option<Vec<f64>>,
}

impl CausalForestOutput {
    /// Get the raw outcome / treatment vectors if they were captured at
    /// training time. Downstream nodes (ATE, BLP) call this.
    pub fn original_outcomes(&self) -> Option<(&[f64], &[f64])> {
        match (&self.y_orig, &self.w_orig) {
            (Some(y), Some(w)) => Some((y.as_slice(), w.as_slice())),
            _ => None,
        }
    }

    /// Populate the raw Y / W vectors. Called by `fit()` internally.
    pub fn set_original_outcomes(&mut self, y: Vec<f64>, w: Vec<f64>) {
        self.y_orig = Some(y);
        self.w_orig = Some(w);
    }
}

pub struct CausalForestFactory;

impl CausalForestFactory {
    pub fn kind() -> &'static str { "grf_causal_forest" }
}

impl CausalForestSpec {
    pub fn fit(&self, batches: &[RecordBatch]) -> Result<CausalForestOutput> {
        let n_rows: usize = batches.iter().map(|b| b.num_rows()).sum();
        if n_rows == 0 {
            return Err(GrfError::Missing("no rows in input batches".into()));
        }
        let schema = batches[0].schema();
        let reserved = [&self.y_column_name, &self.w_column_name];
        let x_cols = if self.x_column_names.is_empty() {
            schema.fields().iter()
                .filter(|f| !reserved.contains(&f.name())
                    && matches!(f.data_type(), DataType::Float64))
                .map(|f| f.name().clone()).collect()
        } else {
            self.x_column_names.clone()
        };
        let x_matrix = arrow_batches_to_matrix(batches, &x_cols, n_rows)?;
        let y = arrow_batches_to_f64(batches, &self.y_column_name, n_rows)?;
        let w = arrow_batches_to_f64(batches, &self.w_column_name, n_rows)?;
        let weights = self.sample_weights_column.as_ref()
            .map(|c| arrow_batches_to_f64(batches, c, n_rows))
            .transpose()?;

        // ── R-learner first stage: estimate Y.hat and W.hat ──
        let (y_hat, w_hat) = match (&self.y_hat, &self.w_hat) {
            (Some(yh), Some(wh)) => (yh.clone(), wh.clone()),
            _ => {
                // Train internal regression forests for the nuisances.
                // Use a smaller forest (max(50, num.trees / 4)) per grf's
                // R convention; ci.group.size = 1 since we only need OOB
                // predictions, no variance estimates.
                let mut nuisance_opts = self.options.to_sys();
                nuisance_opts.num_trees = std::cmp::max(50, self.options.num_trees / 4);
                nuisance_opts.ci_group_size = 1;
                if let Some(_sw) = weights.as_ref() {
                    // Only forward weights if all forests should weight them.
                }
                let y_forest = RegressionTrainer::fit(RegressionSpec {
                    x: x_matrix.clone(), y: y.clone(),
                    sample_weights: weights.clone(), options: nuisance_opts.clone(),
                })?;
                let w_forest = RegressionTrainer::fit(RegressionSpec {
                    x: x_matrix.clone(), y: w.clone(),
                    sample_weights: weights.clone(), options: nuisance_opts,
                })?;
                // Re-collect OOB predictions: the forests' OOB buffers are
                // already populated by grf core when compute_oob_predictions
                // is true. We re-predict here for safety (in case the
                // caller disabled OOB).
                let y_hat = if let Some(oob) = y_forest.oob_predictions() {
                    oob.values
                } else {
                    // Re-predict OOB.
                    use crate::forest::PredictRequest;
                    y_forest.predict(PredictRequest::oob(x_matrix.clone(), x_cols.len(), Some(1)))?.values
                };
                let w_hat = if let Some(oob) = w_forest.oob_predictions() {
                    oob.values
                } else {
                    use crate::forest::PredictRequest;
                    w_forest.predict(PredictRequest::oob(x_matrix.clone(), x_cols.len(), Some(1)))?.values
                };
                (y_hat, w_hat)
            }
        };

        // ── R-learner second stage: train causal forest on residuals ──
        let y_centered: Vec<f64> = y.iter().zip(y_hat.iter()).map(|(yi, yh)| yi - yh).collect();
        let w_centered: Vec<f64> = w.iter().zip(w_hat.iter()).map(|(wi, wh)| wi - wh).collect();
        let trained = CausalTrainer::fit(CausalSpec {
            x: x_matrix,
            y_centered,
            w_centered,
            y_hat: Some(y_hat.clone()),
            w_hat: Some(w_hat.clone()),
            sample_weights: weights,
            stabilize_splits: self.stabilize_splits,
            options: self.options.to_sys(),
        })?;
        let oob = trained.oob_predictions();
        let stats = ForestStats::from(&trained);
        let mut out = CausalForestOutput {
            forest: trained,
            y_hat,
            w_hat,
            oob_predictions: oob,
            stats,
            y_orig: None,
            w_orig: None,
        };
        out.set_original_outcomes(y, w);
        Ok(out)
    }
}

#[allow(dead_code)]
fn _schema() -> SchemaRef {
    Arc::new(Schema::new(vec![Field::new("x", DataType::Float64, false)]))
}

#[allow(dead_code)]
fn _schemars() -> schemars::Schema { schema_for!(CausalForestSpec) }

#[allow(dead_code)]
fn _sys_marker(_: sys::TrainOptions) {}