//! Weighted linear regression through the origin, reproducing R's
//! `lm(y ~ -1 + x1 + ... + xk, weights = w)` and its `summary.lm`.
//!
//! R treats the weights as **precision weights** (inverse variances): the
//! coefficient vector minimises `Σᵢ wᵢ (yᵢ − xᵢ·β)²`, the residual standard
//! error is `sigma = sqrt(Σᵢ wᵢ rᵢ² / (n − p))`, and the coefficient
//! covariance is `sigma² (Xᵀ W X)⁻¹`. Weights are **not** normalised — this
//! reproduces R's `summary.lm` exactly (the same convention as
//! `bio_crates::mr::linalg`).

use faer::linalg::solvers::{DenseSolveCore, Llt, Solve};
use faer::{Mat, Side};

use crate::error::{MrpressoError, Result};

/// Summary of a weighted regression through the origin, matching `summary.lm`
/// with no intercept.
#[derive(Debug, Clone)]
pub struct WlsSummary {
    /// Coefficients, one per exposure, in input order (R's `Estimate`).
    pub coef: Vec<f64>,
    /// Standard error of each coefficient (R's `Std. Error`).
    pub se: Vec<f64>,
    /// Residual standard error `sigma = sqrt(Σ wᵢ rᵢ² / (n − p))`.
    pub sigma: f64,
    /// Residual degrees of freedom `n − p`.
    pub df_resid: f64,
}

impl WlsSummary {
    /// R's `t value` = `Estimate / Std. Error` per coefficient.
    pub fn t_values(&self) -> Vec<f64> {
        self.coef
            .iter()
            .zip(self.se.iter())
            .map(|(b, s)| b / s)
            .collect()
    }

    /// R's `Pr(>|t|)` = `2 * pt(-|t|, df)` per coefficient.
    pub fn p_values(&self) -> Vec<f64> {
        let t = self.t_values();
        t.iter()
            .map(|&tv| pt_two_sided(tv, self.df_resid))
            .collect()
    }
}

/// Two-sided Student-t p-value `2 * pt(|t|, df, lower.tail = FALSE)`.
fn pt_two_sided(t: f64, df: f64) -> f64 {
    match statrs::distribution::StudentsT::new(0.0, 1.0, df) {
        Ok(d) => 2.0 * statrs::distribution::ContinuousCDF::cdf(&d, -t.abs()),
        Err(_) => f64::NAN,
    }
}

/// Fit `lm(y ~ -1 + x1 + ... + xk, weights = w)` — weighted regression through
/// the origin with `p >= 1` exposures. Returns the coefficient vector.
///
/// * `x` — `n × p` design matrix (column `j` is exposure `j`), column-major
///   for faer.
/// * `y` — length-`n` outcome.
/// * `w` — length-`n` precision weights.
pub fn wls_coef(x: &[f64], n: usize, p: usize, y: &[f64], w: &[f64]) -> Result<Vec<f64>> {
    if x.len() != n * p || y.len() != n || w.len() != n {
        return Err(MrpressoError::LengthMismatch(format!(
            "wls_coef: x={} (n*p={}*{}), y={}, w={}",
            x.len(),
            n,
            p,
            y.len(),
            w.len()
        )));
    }
    // Xᵀ W X  (p×p) and Xᵀ W y (p).
    let mut xtwx = vec![0.0; p * p];
    let mut xtwy = vec![0.0; p];
    for i in 0..n {
        let wi = w[i];
        for a in 0..p {
            let xa = x[a * n + i];
            xtwy[a] += wi * xa * y[i];
            for b in 0..p {
                xtwx[a * p + b] += wi * xa * x[b * n + i];
            }
        }
    }
    solve_xtwx_b(xtwx, xtwy, p)
}

/// Solve `beta = (Xᵀ W X)⁻¹ (Xᵀ W y)` for a symmetric PSD Gram matrix.
fn solve_xtwx_b(xtwx: Vec<f64>, xtwy: Vec<f64>, p: usize) -> Result<Vec<f64>> {
    let gram = Mat::from_fn(p, p, |i, j| xtwx[i * p + j]);
    let rhs = Mat::from_fn(p, 1, |i, _| xtwy[i]);
    let llt = Llt::new(gram.as_ref(), faer::Side::Lower)
        .map_err(|e| MrpressoError::Linalg(format!("LLT failed (rank-deficient design?): {e}")))?;
    let sol = llt.solve(&rhs);
    Ok((0..p).map(|i| sol[(i, 0)]).collect())
}

/// Full `summary(lm(y ~ -1 + x, weights = w))` — coefficients, SEs, sigma, and
/// residual df (the pieces MR-PRESSO reports in its `Main MR results` table).
pub fn wls_summary(x: &[f64], n: usize, p: usize, y: &[f64], w: &[f64]) -> Result<WlsSummary> {
    if n <= p {
        return Err(MrpressoError::NotEnoughInstruments(n, p));
    }
    let coef = wls_coef(x, n, p, y, w)?;

    // Weighted residuals rᵢ = yᵢ − xᵢ·β.
    let mut resid = vec![0.0; n];
    for i in 0..n {
        let mut xi = 0.0;
        for a in 0..p {
            xi += x[a * n + i] * coef[a];
        }
        resid[i] = y[i] - xi;
    }

    // sigma² = Σ wᵢ rᵢ² / (n − p);  cov = sigma² (Xᵀ W X)⁻¹.
    let mut ss = 0.0;
    for i in 0..n {
        ss += w[i] * resid[i] * resid[i];
    }
    let df_resid = (n - p) as f64;
    let sigma = (ss / df_resid).sqrt();

    // Invert (Xᵀ W X).
    let mut xtwx = vec![0.0; p * p];
    for i in 0..n {
        let wi = w[i];
        for a in 0..p {
            for b in 0..p {
                xtwx[a * p + b] += wi * x[a * n + i] * x[b * n + i];
            }
        }
    }
    let gram = Mat::from_fn(p, p, |i, j| xtwx[i * p + j]);
    let llt = Llt::new(gram.as_ref(), faer::Side::Lower)
        .map_err(|e| MrpressoError::Linalg(format!("LLT failed (rank-deficient design?): {e}")))?;
    let inv = llt.solve(&Mat::identity(p, p));

    let se = (0..p)
        .map(|a| (sigma * sigma * inv[(a, a)]).sqrt())
        .collect();

    Ok(WlsSummary {
        coef,
        se,
        sigma,
        df_resid,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Cross-check against R: `lm(y ~ -1 + x, weights = w)` for a 3-row case.
    #[test]
    fn wls_through_origin_matches_r() {
        // From R:
        //   y=c(1.2, 2.4, 3.1); x=c(0.5, 1.1, 1.8); w=c(2,1,3)
        //   summary(lm(y ~ -1 + x, weights=w))$coefficients
        //     Estimate Std. Error  t value    Pr(>|t|)
        //   x 1.800525  0.1347687 13.36012 0.005555824
        let y = [1.2, 2.4, 3.1];
        let x = [0.5, 1.1, 1.8];
        let w = [2.0, 1.0, 3.0];
        let s = wls_summary(&x, 3, 1, &y, &w).unwrap();
        assert!((s.coef[0] - 1.800525).abs() < 1e-6);
        assert!((s.se[0] - 0.1347687).abs() < 1e-6);
        assert!((s.t_values()[0] - 13.36012).abs() < 1e-4);
        assert!((s.p_values()[0] - 0.005555824).abs() < 1e-6);
    }
}
