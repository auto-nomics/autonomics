//! DAG node: `grf_average_treatment_effect`
//!
//! Computes doubly-robust (AIPW) average treatment effect estimates from a
//! trained `causal_forest`. Mirrors R's `average_treatment_effect`:
//!
//! ```text
//!   DR_score[i] = τ̂[i] + γ[i] · (Y[i] − Ŷ[i] − τ̂[i] · (W[i] − Ŵ[i]))
//!   γ[i]       = (W[i] − Ŵ[i]) / (Ŵ[i] · (1 − Ŵ[i]))     (binary treatment)
//   estimate    = weighted_mean(DR_score)
//   SE          = sqrt(cluster_robust_var(DR_score))
//! ```
//!
//! Supports `target.sample` ∈ {all, treated, control, overlap}.

use std::sync::Arc;

use arrow_array::{Array, Float64Array, RecordBatch};
use arrow_schema::{DataType, Field, Schema, SchemaRef};
use schemars::{JsonSchema, schema_for};
use serde::{Deserialize, Serialize};

use crate::forest::{ForestBlob, ForestKind, OobPredictions};
use crate::nodes::causal_forest::CausalForestOutput;
use crate::nodes::dr_scores::dr_scores_binary;
use crate::nodes::regression_forest::arrow_batches_to_f64;
use crate::{GrfError, Result};

/// Mirrors `grf::average_treatment_effect(forest, target.sample, method, subset, ...)`.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct AverageTreatmentEffectSpec {
    /// Which sample to aggregate over. `all`/`treated`/`control`/`overlap`.
    /// Mirrors grf R. Default `all`.
    #[serde(default = "default_target_sample")]
    pub target_sample: String,
    /// Inference method. Currently only `AIPW` is implemented (TMLE is grf R
    /// only, and we don't replicate it yet).
    #[serde(default = "default_method")]
    pub method: String,
    /// Optional boolean mask (length == n_train) for restricting the analysis.
    #[serde(default)]
    pub subset: Option<Vec<bool>>,
    /// Optional clusters for cluster-robust SEs. `None` → no clustering.
    #[serde(default)]
    pub clusters: Option<Vec<i64>>,
}

fn default_target_sample() -> String {
    "all".into()
}
fn default_method() -> String {
    "AIPW".into()
}

#[derive(Debug, Clone, Serialize)]
pub struct AverageTreatmentEffectOutput {
    pub estimate: f64,
    pub std_err: f64,
    pub target_sample: String,
    pub method: String,
    pub n_effective: usize,
}

pub struct AverageTreatmentEffectFactory;

impl AverageTreatmentEffectFactory {
    pub fn kind() -> &'static str {
        "grf_average_treatment_effect"
    }
}

impl AverageTreatmentEffectSpec {
    /// Compute the AIPW estimate. Consumes the full output of `grf_causal_forest`
    /// (which carries Y.orig, W.orig, Ŷ, Ŵ, τ̂), so this node slots in
    /// downstream of the causal forest trainer.
    pub fn estimate(&self, causal: &CausalForestOutput) -> Result<AverageTreatmentEffectOutput> {
        if causal.forest.kind() != ForestKind::Causal {
            return Err(GrfError::KindMismatch {
                trained: causal.forest.kind(),
                requested: ForestKind::Causal,
            });
        }
        let y_hat = &causal.y_hat;
        let w_hat = &causal.w_hat;
        let tau_hat = causal
            .oob_predictions
            .as_ref()
            .ok_or_else(|| {
                GrfError::Missing(
                    "forest missing OOB tau predictions; retrain with compute_oob_predictions=true"
                        .into(),
                )
            })?
            .values
            .clone();

        // We need the original Y and W here. The DAG layer would carry these
        // alongside the CausalForestOutput; for the bio_crates library they
        // are not stored by default. We require the caller to provide them
        // via a parallel call.
        let (y_orig, w_orig) = match causal.original_outcomes() {
            Some((y, w)) => (y, w),
            None => return Err(GrfError::Missing(
                "CausalForestOutput missing original Y/W; pass them via set_original_outcomes() \
                 or wire a column-carrying DAG input port"
                    .into(),
            )),
        };

        let n = y_orig.len();
        let weights = vec![1.0_f64; n];

        // AIPW scores (binary treatment form; grf R's `get_scores.causal_forest`).
        let dr = dr_scores_binary(y_orig, w_orig, y_hat, w_hat, &tau_hat);

        // Subset.
        let mask = self
            .subset
            .as_ref()
            .cloned()
            .unwrap_or_else(|| vec![true; n]);
        if mask.len() != n {
            return Err(GrfError::Shape("subset length mismatch".into()));
        }

        // target.sample filter.
        let target = self.target_sample.as_str();
        let include: Vec<bool> = (0..n)
            .map(|i| {
                if !mask[i] {
                    return false;
                }
                match target {
                    "all" => true,
                    "treated" => w_orig[i] == 1.0,
                    "control" => w_orig[i] == 0.0,
                    "overlap" => w_hat[i] > 0.0 && w_hat[i] < 1.0,
                    _ => return false, // unknown target → silently skip; grf R would error
                }
            })
            .collect();

        let dr_f: Vec<f64> = dr
            .iter()
            .zip(include.iter())
            .filter(|x| *x.1)
            .map(|x| *x.0)
            .collect();
        let w_f: Vec<f64> = weights
            .iter()
            .zip(include.iter())
            .filter(|x| *x.1)
            .map(|x| *x.0)
            .collect();
        let n_eff = dr_f.len();
        if n_eff == 0 {
            return Err(GrfError::Shape(
                "subset / target.sample produced 0 observations".into(),
            ));
        }

        // Weighted mean.
        let sum_w: f64 = w_f.iter().sum();
        let tau_hat_mean = dr_f.iter().zip(w_f.iter()).map(|(d, w)| d * w).sum::<f64>() / sum_w;

        // Cluster-robust SE.
        let clusters: Vec<i64> = match self.clusters.as_ref() {
            Some(c) => c.clone(),
            None => (0..n_eff as i64).collect(),
        };
        let tau_se = cluster_robust_se(&dr_f, &w_f, &clusters, tau_hat_mean)?;

        Ok(AverageTreatmentEffectOutput {
            estimate: tau_hat_mean,
            std_err: tau_se,
            target_sample: self.target_sample.clone(),
            method: self.method.clone(),
            n_effective: n_eff,
        })
    }
}

/// Cluster-robust variance: σ̂² = (n/(n-1)) · Σ_g (Σ_{i∈g} wᵢ (DRᵢ − τ̂))² / (Σ wᵢ)².
fn cluster_robust_se(dr: &[f64], w: &[f64], clusters: &[i64], tau_mean: f64) -> Result<f64> {
    let n = dr.len();
    if clusters.len() != n {
        return Err(GrfError::Shape("clusters length mismatch".into()));
    }
    let sum_w: f64 = w.iter().sum();
    let mut cluster_dev: std::collections::HashMap<i64, f64> = std::collections::HashMap::new();
    for i in 0..n {
        let g = clusters[i];
        let d = w[i] * (dr[i] - tau_mean);
        *cluster_dev.entry(g).or_insert(0.0) += d;
    }
    let n_clusters = cluster_dev.len();
    if n_clusters <= 1 {
        return Err(GrfError::Shape("not enough clusters for robust SE".into()));
    }
    let sum_sq: f64 = cluster_dev.values().map(|v| v * v).sum();
    let var = sum_sq / (sum_w * sum_w) * (n_clusters as f64 / (n_clusters as f64 - 1.0));
    Ok(var.sqrt())
}

#[allow(dead_code)]
fn _schema() -> SchemaRef {
    Arc::new(Schema::new(vec![Field::new(
        "estimate",
        DataType::Float64,
        false,
    )]))
}

#[allow(dead_code)]
fn _schemars() -> schemars::Schema {
    schema_for!(AverageTreatmentEffectSpec)
}

#[allow(dead_code)]
fn _oob_marker(_: OobPredictions) {}
