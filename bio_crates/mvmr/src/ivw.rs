//! IVW multivariable MR — faithful port of `R/ivw_mvmr.R`.
//!
//! Fits the multivariable IVW model
//! ```text
//!   β_YG = β₁·β_X1 + β₂·β_X2 + … + β_p·β_Xp + ε
//! ```
//! through the origin with first-order inverse-variance weights
//! `w_j = 1 / se(β_YG)²`. The coefficient table (estimate, SE, t-stat,
//! two-sided p-value) reproduces R's `summary(lm(betaYG ~ -1 + betaX*,
//! weights = Wj))`.

use crate::error::Result;
use crate::format::MvmrInput;
use crate::linalg::{WlsSummary, wls_origin};

/// Output of [`ivw_mvmr`], matching the coefficient matrix returned by R's
/// `ivw_mvmr()` (Estimate / Std. Error / t value / Pr(>|t|)).
#[derive(Debug, Clone)]
pub struct IvwResult {
    /// Per-exposure point estimate (length `p`).
    pub estimate: Vec<f64>,
    /// Per-exposure standard error.
    pub se: Vec<f64>,
    /// Per-exposure t-statistic.
    pub t_stat: Vec<f64>,
    /// Per-exposure two-sided p-value.
    pub pvalue: Vec<f64>,
    /// Residual standard error from `summary.lm`'s `sigma`.
    pub sigma: f64,
    /// Residual degrees of freedom `n − p`.
    pub df_resid: f64,
    /// Underlying weighted regression summary (kept for downstream
    /// strength/pleiotropy computations).
    pub fit: WlsSummary,
}

impl IvwResult {
    pub fn n_exposures(&self) -> usize {
        self.estimate.len()
    }
}

/// Fit the IVW multivariable MR model.
///
/// Equivalent to R's `ivw_mvmr(r_input)` with default `gencov = 0` (the only
/// value that affects the IVW point estimate — the R docs note `gencov` is
/// retained only for interface consistency).
pub fn ivw_mvmr(input: &MvmrInput) -> Result<IvwResult> {
    input.validate()?;
    let n = input.n_snps();
    let p = input.n_exposures();

    // Precision weights: Wj <- 1 / r_input[, 3]^2.
    let w: Vec<f64> = (0..n).map(|i| 1.0 / input.sebeta_yg[i].powi(2)).collect();

    // Design matrix: the exposure-effect columns (betaX1..betaXp).
    let x: Vec<Vec<f64>> = (0..n)
        .map(|i| (0..p).map(|j| input.beta_xg[j][i]).collect())
        .collect();

    let fit = wls_origin(&x, &input.beta_yg, &w)?;

    Ok(IvwResult {
        estimate: fit.coef.clone(),
        se: fit.se.clone(),
        t_stat: fit.t_stat.clone(),
        pvalue: fit.pvalue.clone(),
        sigma: fit.sigma,
        df_resid: fit.df_resid,
        fit,
    })
}
