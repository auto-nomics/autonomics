//! DAG node: `grf_test_calibration`
//!
//! Omnibus calibration test for a forest, mirroring R's `test_calibration`.
//!
//! ```text
//!   For a regression forest:
//!     target = Y - Ȳ
//!     regress target ~ mean_pred + (pred - mean_pred), cluster-robust SE
//!     coefficient on (pred - mean_pred) is the heterogeneity test
//!
//!   For a causal forest:
//!     target = (Y - Ŷ)
//!     regress target ~ mean(W-Ŵ)·τ̂ + (W-Ŵ)·(τ̂ - mean(τ̂)), HC SE
//! ```

use std::sync::Arc;

use arrow_schema::{DataType, Field, Schema, SchemaRef};
use schemars::{schema_for, JsonSchema};
use serde::{Deserialize, Serialize};
use statkit::regression::wls;

use crate::forest::ForestKind;
use crate::nodes::regression_forest::arrow_batches_to_f64;
use crate::{GrfError, Result};
use grf_sys as sys;

/// Mirrors `grf::test_calibration(forest)`.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct TestCalibrationSpec {
    /// Optional override for the HC variant. Defaults to "HC3" (matches grf R).
    /// Currently we always compute HC0-robust SE (one-sided p-values).
    #[serde(default = "default_vcov")]
    pub vcov_type: String,
}

fn default_vcov() -> String { "HC3".into() }

#[derive(Debug, Clone, Serialize)]
pub struct TestCalibrationOutput {
    pub mean_pred_coefficient: f64,
    pub mean_pred_se: f64,
    pub mean_pred_p_value: f64,
    pub differential_pred_coefficient: f64,
    pub differential_pred_se: f64,
    pub differential_pred_p_value: f64,
    pub n_obs: usize,
}

pub struct TestCalibrationFactory;

impl TestCalibrationFactory {
    pub fn kind() -> &'static str { "grf_test_calibration" }
}

impl TestCalibrationSpec {
    /// Run the calibration check against a trained causal forest. Mirrors
    /// grf R's calibration test for `causal_forest` (the regression variant
    /// is a simpler sub-case).
    pub fn check_causal(&self, causal: &crate::nodes::causal_forest::CausalForestOutput) -> Result<TestCalibrationOutput> {
        if causal.forest.kind() != ForestKind::Causal {
            return Err(GrfError::KindMismatch {
                trained: causal.forest.kind(),
                requested: ForestKind::Causal,
            });
        }
        let (y_orig, w_orig) = causal.original_outcomes()
            .ok_or_else(|| GrfError::Missing("missing Y/W on causal forest output".into()))?;
        let y_hat = &causal.y_hat;
        let w_hat = &causal.w_hat;
        let tau_hat = causal.oob_predictions.as_ref()
            .ok_or_else(|| GrfError::Missing("missing OOB tau".into()))?
            .values
            .clone();

        let n = y_orig.len();
        let mean_tau: f64 = tau_hat.iter().sum::<f64>() / n as f64;
        // target = Y - Y.hat
        let target: Vec<f64> = y_orig.iter().zip(y_hat.iter()).map(|(y, yh)| y - yh).collect();
        // Regressors:
        //   x1 = (W - W.hat) * mean_tau
        //   x2 = (W - W.hat) * (tau - mean_tau)
        let x1: Vec<f64> = (0..n).map(|i| (w_orig[i] - w_hat[i]) * mean_tau).collect();
        let x2: Vec<f64> = (0..n).map(|i| (w_orig[i] - w_hat[i]) * (tau_hat[i] - mean_tau)).collect();
        let cols: Vec<&[f64]> = vec![&x1, &x2];
        let fit = wls(&cols, &target, &vec![1.0; n], /*intercept=*/false)
            .map_err(|e| GrfError::Cpp(format!("statkit::wls: {e}")))?;
        Ok(TestCalibrationOutput {
            mean_pred_coefficient: fit.coefficients[0],
            mean_pred_se: fit.std_errors[0],
            mean_pred_p_value: one_sided(fit.t_stats[0], fit.p_values[0]),
            differential_pred_coefficient: fit.coefficients[1],
            differential_pred_se: fit.std_errors[1],
            differential_pred_p_value: one_sided(fit.t_stats[1], fit.p_values[1]),
            n_obs: n,
        })
    }
}

/// grf R's calibration test converts two-sided p-values to one-sided
/// (testing H₁: coefficient > 0). Replicate that.
fn one_sided(t: f64, p_two_sided: f64) -> f64 {
    if t < 0.0 { 1.0 - p_two_sided / 2.0 } else { p_two_sided / 2.0 }
}

#[allow(dead_code)]
fn _schema() -> SchemaRef {
    Arc::new(Schema::new(vec![Field::new("p_value", DataType::Float64, false)]))
}

#[allow(dead_code)]
fn _schemars() -> schemars::Schema { schema_for!(TestCalibrationSpec) }

#[allow(dead_code)]
fn _sys_marker(_: sys::TrainOptions) {}