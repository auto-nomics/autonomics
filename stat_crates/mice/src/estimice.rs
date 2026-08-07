//! `estimice` — Bayesian linear regression draws for mice.
//!
//! A faithful Rust port of `mice:::.norm.draw` (`R/mice.impute.norm.R`). Used
//! by [`crate::pmm`] (predictive mean matching) and [`crate::norm`] (Bayesian
//! linear regression imputation) — both imputation methods share the same
//! underlying regression draw.
//!
//! Reproduces R's `estimice()` output to ~1e-12: the coefficient estimate
//! `c`, the Bayesian draw `β*`, the posterior scale `σ*`, and the chosen
//! least-squares method (`"qr"` or `"ridge"`).

use rand::Rng;
use rand_distr::{ChiSquared, Distribution, Normal};

use crate::error::Result;
use crate::linalg::{QrFit, RidgeFit, chol_factor, qr_fit, ridge_fit, symmetrise};

/// Parameters drawn from the Bayesian linear regression posterior.
#[derive(Debug, Clone)]
pub struct NormDraw {
    /// OLS coefficient estimate β̂ (length `p`).
    pub coef: Vec<f64>,
    /// Posterior draw β* = β̂ + σ* · L · z, where `L = chol(sym(V))` and
    /// `z ~ N(0, I_p)` (length `p`).
    pub beta: Vec<f64>,
    /// Posterior draw `σ* = √(RSS / χ²_ν)` with `ν = df_residual` (scalar).
    pub sigma: f64,
    /// Residual degrees of freedom.
    pub df: usize,
    /// Residuals r = y − Xβ̂.
    pub residuals: Vec<f64>,
}

/// Reproduce `mice:::.norm.draw(y, ry, x, ridge = 1e-5, ls.meth = "qr")`.
///
/// Inputs:
/// * `y_obs` — observed response values, length `n_ry`.
/// * `x_obs` — observed design matrix, `n_ry × p` row-major, **with** an
///   intercept column prepended (mirroring R's `cbind(1, x)` convention used
///   by all imputation methods).
/// * `ridge` — ridge penalty for the Gram matrix inverse (`1e-5` default in
///   `estimice`).
/// * `ls_meth` — `"qr"` (default, mirrors `lm.fit`) or `"ridge"`.
/// * `rng` — caller-owned RNG (so the orchestrator can replay a sequence).
pub fn norm_draw<R: Rng + ?Sized>(
    y_obs: &[f64],
    x_obs: &[Vec<f64>],
    ridge: f64,
    ls_meth: &str,
    rng: &mut R,
) -> Result<NormDraw> {
    // Step 1-2: fit OLS or ridge-regression and obtain β̂, V, residuals.
    let (coef, v, residuals, df): (Vec<f64>, Vec<f64>, Vec<f64>, usize) = match ls_meth {
        "qr" => {
            let fit: QrFit = qr_fit(x_obs, y_obs)?;
            (fit.coef, fit.v, fit.residuals, fit.df)
        }
        "ridge" => {
            let fit: RidgeFit = ridge_fit(x_obs, y_obs, ridge)?;
            (fit.coef, fit.v, fit.residuals, y_obs.len().saturating_sub(x_obs[0].len()))
        }
        other => {
            return Err(crate::error::MiceError::InvalidSpec(format!(
                "estimice: unknown ls.meth '{other}'"
            )))
        }
    };

    let p = coef.len();

    // Step 3: σ* = √(Σ rᵢ² / χ²_ν).
    let rss: f64 = residuals.iter().map(|r| r * r).sum();
    let chi = ChiSquared::new(df as f64).map_err(|e| {
        crate::error::MiceError::Numerical(format!("chi-squared df: {e}"))
    })?;
    let g = chi.sample(rng);
    let sigma = (rss / g).sqrt();

    // Step 4-5: L = chol(sym(V)) (Cholesky of symmetrised V), then
    // β* = β̂ + σ* · L · z with z ~ N(0, I_p).
    let sym_v = symmetrise(&v);
    let l = chol_factor(&sym_v)?;
    let normal = Normal::new(0.0, 1.0).map_err(|e| {
        crate::error::MiceError::Numerical(format!("normal init: {e}"))
    })?;
    let z: Vec<f64> = (0..p).map(|_| normal.sample(rng)).collect();

    // Compute L · z.
    let mut lz = vec![0.0_f64; p];
    let p_dim = p;
    for j in 0..p {
        let mut s = 0.0;
        for k in 0..p_dim {
            s += l[j * p + k] * z[k];
        }
        lz[j] = s;
    }

    let beta: Vec<f64> = (0..p).map(|j| coef[j] + sigma * lz[j]).collect();

    Ok(NormDraw {
        coef,
        beta,
        sigma,
        df,
        residuals,
    })
}
