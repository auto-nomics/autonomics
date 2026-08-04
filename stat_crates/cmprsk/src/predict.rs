//! `predict.crr` — predicted cumulative incidence at given covariate values.
//!
//! Port of `reference/cmprsk/R/cmprsk.R:285-326`.
//!
//! The predicted subdistribution is
//! `F(t; z) = 1 - exp(-Λ̂₀(t) · exp(z'β))`, where `Λ̂₀` is the cumulative sum of
//! the Breslow-type jumps in [`CrrFit::bfitj`](crate::CrrFit::bfitj). With
//! time-interacted covariates the linear predictor varies over the failure
//! times, so the exponential goes *inside* the cumulative sum — exactly as the
//! R code writes `cumsum(exp(tfs %*% ...) * bfitj)`.

use crate::crr::CrrFit;
use crate::error::{CmprskError, Result};

/// Predicted cumulative incidence curves.
#[derive(Debug, Clone)]
pub struct CrrPrediction {
    /// Unique failure times (the curve's x-axis).
    pub uftime: Vec<f64>,
    /// One curve per covariate row: `curves[j][k]` is `F(uftime[k])`.
    pub curves: Vec<Vec<f64>>,
}

/// Predict cumulative incidence for each row of `cov1` / `cov2`.
///
/// `cov1` must have `fit.ncov1` columns and `cov2` must have `fit.ncov2`
/// columns; both are row-major with one row per covariate combination. Pass an
/// empty slice for a block the model does not use.
pub fn predict_crr(fit: &CrrFit, cov1: &[Vec<f64>], cov2: &[Vec<f64>]) -> Result<CrrPrediction> {
    let ndf = fit.uftime.len();
    let nc1 = fit.ncov1;
    let nc2 = fit.ncov2;

    let m = if nc1 > 0 { cov1.len() } else { cov2.len() };
    if m == 0 {
        return Err(CmprskError::Predict("no covariate rows supplied".into()));
    }
    if nc1 > 0 && (cov1.len() != m || cov1.iter().any(|r| r.len() != nc1)) {
        return Err(CmprskError::Predict(format!(
            "cov1 must be {m} × {nc1}"
        )));
    }
    if nc2 > 0 && (cov2.len() != m || cov2.iter().any(|r| r.len() != nc2)) {
        return Err(CmprskError::Predict(format!(
            "cov2 must be {m} × {nc2}"
        )));
    }

    let mut curves = Vec::with_capacity(m);
    for j in 0..m {
        // Fixed part of the linear predictor.
        let fixed: f64 = (0..nc1).map(|k| cov1[j][k] * fit.coef[k]).sum();
        let mut lhat = Vec::with_capacity(ndf);
        let mut acc = 0.0_f64;
        for k in 0..ndf {
            // Time-varying part: tfs[k, ·] · (cov2[j, ·] * coef_tail).
            let tv: f64 = (0..nc2)
                .map(|c| fit.tfs[k][c] * cov2[j][c] * fit.coef[nc1 + c])
                .sum();
            acc += (fixed + tv).exp() * fit.bfitj[k];
            lhat.push(1.0 - (-acc).exp());
        }
        curves.push(lhat);
    }

    Ok(CrrPrediction {
        uftime: fit.uftime.clone(),
        curves,
    })
}

/// Baseline cumulative incidence — [`predict_crr`] at all covariates zero.
///
/// Equivalent to `1 - exp(-cumsum(bfitj))`, which is what the DAG node exposes
/// on its second output port.
pub fn baseline_cif(fit: &CrrFit) -> Vec<f64> {
    let mut acc = 0.0_f64;
    fit.bfitj
        .iter()
        .map(|j| {
            acc += j;
            1.0 - (-acc).exp()
        })
        .collect()
}
