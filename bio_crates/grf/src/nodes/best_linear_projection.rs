//! DAG node: `grf_best_linear_projection`
//!
//! Doubly-robust best linear projection of the CATE onto a set of features A.
//!
//! ```text
//!   DR_score[i] = τ̂[i] + γ[i] · (Y[i] − Ŷ[i] − τ̂[i] · (W[i] − Ŵ[i]))
//!   WLS: DR_score ~ β₀ + A · β
//! ```
//!
//! Mirrors R's `best_linear_projection(forest, A, ...)`.

use std::sync::Arc;

use arrow_array::{Array, Float64Array, RecordBatch};
use arrow_schema::{DataType, Field, Schema, SchemaRef};
use schemars::{JsonSchema, schema_for};
use serde::{Deserialize, Serialize};
use statkit::regression::wls;

use crate::forest::ForestKind;
use crate::nodes::causal_forest::CausalForestOutput;
use crate::nodes::dr_scores::dr_scores_binary;
use crate::nodes::regression_forest::arrow_batches_to_f64;
use crate::{GrfError, Result};

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct BestLinearProjectionSpec {
    /// Names of columns used as A (the projection features). If empty, no A
    /// is included (returns just the intercept, equivalent to the AIPW
    /// average treatment effect).
    #[serde(default)]
    pub a_column_names: Vec<String>,
    /// Subset mask (length == n_train).
    #[serde(default)]
    pub subset: Option<Vec<bool>>,
    /// Sample weights. `None` → uniform.
    #[serde(default)]
    pub sample_weights: Option<Vec<f64>>,
}

#[derive(Debug, Clone, Serialize)]
pub struct BestLinearProjectionOutput {
    /// Coefficient estimates. Index 0 is the intercept; the rest correspond
    /// to `a_column_names` in order.
    pub coefficients: Vec<f64>,
    /// HC-robust standard errors (same length as `coefficients`).
    pub std_errors: Vec<f64>,
    /// Two-sided t-stats.
    pub t_stats: Vec<f64>,
    /// Two-sided p-values.
    pub p_values: Vec<f64>,
}

pub struct BestLinearProjectionFactory;

impl BestLinearProjectionFactory {
    pub fn kind() -> &'static str {
        "grf_best_linear_projection"
    }
}

impl BestLinearProjectionSpec {
    /// Project the CATE onto A.
    /// `a_batches` is the port-1 RecordBatch carrying the A columns; pass
    /// `None` to project on the intercept only (ATE).
    pub fn project(
        &self,
        causal: &CausalForestOutput,
        a_batches: Option<&[RecordBatch]>,
    ) -> Result<BestLinearProjectionOutput> {
        if causal.forest.kind() != ForestKind::Causal {
            return Err(GrfError::KindMismatch {
                trained: causal.forest.kind(),
                requested: ForestKind::Causal,
            });
        }
        let (y_orig, w_orig) = causal
            .original_outcomes()
            .ok_or_else(|| GrfError::Missing("missing Y/W on causal forest output".into()))?;
        let y_hat = &causal.y_hat;
        let w_hat = &causal.w_hat;
        let tau_hat = causal
            .oob_predictions
            .as_ref()
            .ok_or_else(|| GrfError::Missing("missing OOB tau".into()))?
            .values
            .clone();

        let n = y_orig.len();
        let dr = dr_scores_binary(y_orig, w_orig, y_hat, w_hat, &tau_hat);

        // Subset mask.
        let mask = self
            .subset
            .as_ref()
            .cloned()
            .unwrap_or_else(|| vec![true; n]);
        if mask.len() != n {
            return Err(GrfError::Shape("subset length mismatch".into()));
        }

        // A matrix (column-major).
        let a_columns: Vec<Vec<f64>> = match a_batches {
            Some(batches) => {
                let n_a: usize = batches.iter().map(|b| b.num_rows()).sum();
                if n_a != n {
                    return Err(GrfError::Shape("A rows != n_train".into()));
                }
                self.a_column_names
                    .iter()
                    .map(|name| arrow_batches_to_f64(batches, name, n))
                    .collect::<Result<Vec<_>>>()?
            }
            None => Vec::new(),
        };
        let p = a_columns.len();

        // Build the design: column-major with 1 (intercept) + a_columns.
        let mut x_buf: Vec<f64> = Vec::with_capacity((1 + p) * n);
        // intercept first column (all ones).
        x_buf.extend(std::iter::repeat_n(1.0, n));
        for col in &a_columns {
            x_buf.extend_from_slice(col);
        }
        // column-major row pointers.
        let cols: Vec<&[f64]> = (0..(1 + p)).map(|j| &x_buf[j * n..(j + 1) * n]).collect();

        let dr_f: Vec<f64> = dr
            .iter()
            .zip(mask.iter())
            .filter(|x| *x.1)
            .map(|x| *x.0)
            .collect();
        let n_eff = dr_f.len();
        if n_eff == 0 {
            return Err(GrfError::Shape("subset produced 0 rows".into()));
        }
        let cols_f: Vec<Vec<f64>> = cols
            .iter()
            .map(|c| {
                c.iter()
                    .zip(mask.iter())
                    .filter(|x| *x.1)
                    .map(|x| *x.0)
                    .collect()
            })
            .collect();
        let cols_f_refs: Vec<&[f64]> = cols_f.iter().map(|c| c.as_slice()).collect();

        // Uniform weights (grf R applies no sample weights in BLP unless given).
        let weights: Vec<f64> = match self.sample_weights.as_ref() {
            None => vec![1.0; n_eff],
            Some(w) => {
                if w.len() != n {
                    return Err(GrfError::Shape("weights length mismatch".into()));
                }
                w.iter()
                    .zip(mask.iter())
                    .filter(|x| *x.1)
                    .map(|x| *x.0)
                    .collect()
            }
        };

        let fit = wls(&cols_f_refs, &dr_f, &weights, /*intercept=*/ false)
            .map_err(|e| GrfError::Cpp(format!("statkit::wls: {e}")))?;
        Ok(BestLinearProjectionOutput {
            coefficients: fit.coefficients,
            std_errors: fit.std_errors,
            t_stats: fit.t_stats,
            p_values: fit.p_values,
        })
    }
}

#[allow(dead_code)]
fn _schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new("coefficient", DataType::Float64, false),
        Field::new("std_error", DataType::Float64, false),
    ]))
}

#[allow(dead_code)]
fn _schemars() -> schemars::Schema {
    schema_for!(BestLinearProjectionSpec)
}

// Sentinel re-export to silence unused-import warnings on shared helpers.
#[allow(dead_code)]
fn _ate_marker_alias(_: f64) {}

#[allow(unused_imports)]
use crate::nodes::average_treatment_effect::AverageTreatmentEffectFactory as _AteReExport;
