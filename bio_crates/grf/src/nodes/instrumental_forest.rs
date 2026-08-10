//! DAG node: `grf_instrumental_forest`
//!
//! Trains an instrumental-variable forest that estimates local average
//! treatment effects using a binary or continuous instrument Z:
//!
//! ```text
//!   τ(x) = Cov[Y, Z | X=x] / Cov[W, Z | X=x]
//! ```
//!
//! Mirrors grf R's `instrumental_forest(X, Y, W, Z, Y.hat, W.hat, Z.hat)`.

use std::sync::Arc;

use arrow_array::{Array, Float64Array, RecordBatch};
use arrow_schema::{DataType, Field, Schema, SchemaRef};
use schemars::{schema_for, JsonSchema};
use serde::{Deserialize, Serialize};

use crate::data::Matrix;
use crate::forest::{
    ForestBlob, ForestStats, InstrumentalSpec, InstrumentalTrainer, OobPredictions,
};
use crate::nodes::regression_forest::{
    arrow_batches_to_f64, arrow_batches_to_matrix, NodeTrainOptions,
};
use crate::{GrfError, Result};
use grf_sys as sys;

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct InstrumentalForestSpec {
    /// Names of columns in `port 0` used as features.
    #[serde(default)]
    pub x_column_names: Vec<String>,
    /// Name of the outcome column.
    pub y_column_name: String,
    /// Name of the treatment column.
    pub w_column_name: String,
    /// Name of the instrument column.
    pub z_column_name: String,
    /// Pre-computed Ŷ(X); if omitted, trained internally via regression forest.
    #[serde(default)]
    pub y_hat: Option<Vec<f64>>,
    /// Pre-computed Ŵ(X).
    #[serde(default)]
    pub w_hat: Option<Vec<f64>>,
    /// Pre-computed Ẑ(X).
    #[serde(default)]
    pub z_hat: Option<Vec<f64>>,
    /// Reduced-form weight in the GRF IV objective. Default 0.0 (matches
    /// grf R's default).
    #[serde(default)]
    pub reduced_form_weight: f64,
    /// Whether to take treatment assignment into account when determining
    /// the imbalance of a split.
    #[serde(default = "default_true")]
    pub stabilize_splits: bool,
    /// Optional per-sample weights column.
    #[serde(default)]
    pub sample_weights_column: Option<String>,
    /// Forest hyperparameters.
    #[serde(default)]
    pub options: NodeTrainOptions,
}

fn default_true() -> bool { true }

#[derive(Debug, Clone)]
pub struct InstrumentalForestOutput {
    pub forest: ForestBlob,
    /// Ŷ(X) used by the IV forest. Auto-trained if not given.
    pub y_hat: Vec<f64>,
    /// Ŵ(X).
    pub w_hat: Vec<f64>,
    /// Ẑ(X).
    pub z_hat: Vec<f64>,
    pub oob_predictions: Option<OobPredictions>,
    pub stats: ForestStats,
}

pub struct InstrumentalForestFactory;

impl InstrumentalForestFactory {
    pub fn kind() -> &'static str { "grf_instrumental_forest" }
}

impl InstrumentalForestSpec {
    pub fn fit(&self, batches: &[RecordBatch]) -> Result<InstrumentalForestOutput> {
        let n_rows: usize = batches.iter().map(|b| b.num_rows()).sum();
        if n_rows == 0 {
            return Err(GrfError::Missing("no rows in input batches".into()));
        }
        let schema = batches[0].schema();
        let reserved = [&self.y_column_name, &self.w_column_name, &self.z_column_name];
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
        let z = arrow_batches_to_f64(batches, &self.z_column_name, n_rows)?;
        let weights = self.sample_weights_column.as_ref()
            .map(|c| arrow_batches_to_f64(batches, c, n_rows))
            .transpose()?;

        // R-learner-style nuisance forests (only if user didn't provide them).
        let (y_hat, w_hat, z_hat) = match (&self.y_hat, &self.w_hat, &self.z_hat) {
            (Some(yh), Some(wh), Some(zh)) => (yh.clone(), wh.clone(), zh.clone()),
            _ => {
                let mut opts = self.options.to_sys();
                opts.num_trees = std::cmp::max(50, self.options.num_trees / 4);
                opts.ci_group_size = 1;
                let y_forest = crate::forest::RegressionTrainer::fit(
                    crate::forest::RegressionSpec {
                        x: x_matrix.clone(), y: y.clone(),
                        sample_weights: weights.clone(), options: opts.clone(),
                    })?;
                let w_forest = crate::forest::RegressionTrainer::fit(
                    crate::forest::RegressionSpec {
                        x: x_matrix.clone(), y: w.clone(),
                        sample_weights: weights.clone(), options: opts.clone(),
                    })?;
                let z_forest = crate::forest::RegressionTrainer::fit(
                    crate::forest::RegressionSpec {
                        x: x_matrix.clone(), y: z.clone(),
                        sample_weights: weights.clone(), options: opts,
                    })?;
                let yh = regression_oob(&y_forest, &x_matrix, x_cols.len())?;
                let wh = regression_oob(&w_forest, &x_matrix, x_cols.len())?;
                let zh = regression_oob(&z_forest, &x_matrix, x_cols.len())?;
                (yh, wh, zh)
            }
        };

        let y_centered: Vec<f64> = y.iter().zip(y_hat.iter()).map(|(yi, yh)| yi - yh).collect();
        let w_centered: Vec<f64> = w.iter().zip(w_hat.iter()).map(|(wi, wh)| wi - wh).collect();
        let z_centered: Vec<f64> = z.iter().zip(z_hat.iter()).map(|(zi, zh)| zi - zh).collect();

        let trained = InstrumentalTrainer::fit(InstrumentalSpec {
            x: x_matrix,
            y_centered,
            w_centered,
            z_centered,
            sample_weights: weights,
            reduced_form_weight: self.reduced_form_weight,
            stabilize_splits: self.stabilize_splits,
            options: self.options.to_sys(),
        })?;

        let oob = trained.oob_predictions();
        let stats = ForestStats::from(&trained);
        Ok(InstrumentalForestOutput {
            forest: trained,
            y_hat, w_hat, z_hat,
            oob_predictions: oob,
            stats,
        })
    }
}

/// Re-collect OOB predictions from a regression forest.
fn regression_oob(forest: &ForestBlob, x_matrix: &Matrix, n_features: usize) -> Result<Vec<f64>> {
    use crate::forest::PredictRequest;
    let req = PredictRequest::oob(x_matrix.clone(), n_features, Some(1));
    Ok(forest.predict(req)?.values)
}

#[allow(dead_code)]
fn _schema() -> SchemaRef {
    Arc::new(Schema::new(vec![Field::new("x", DataType::Float64, false)]))
}

#[allow(dead_code)]
fn _schemars() -> schemars::Schema { schema_for!(InstrumentalForestSpec) }

#[allow(dead_code)]
fn _sys_marker(_: sys::TrainOptions) {}