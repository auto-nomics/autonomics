//! Multivariable weighted linear regression through the origin, faithful to
//! R's `lm(y ~ -1 + X, weights = w)` + `summary.lm`.
//!
//! R's `lm` treats the supplied weights as precision weights: the coefficient
//! minimises `Σ wᵢ (yᵢ − xᵢ·β)²`, the residual standard error is
//! `sigma = sqrt(Σ wᵢ rᵢ² / (n − p))`, and the coefficient covariance is
//! `sigma² (Xᵀ W X)⁻¹`. Weights are **not** normalised.
//!
//! This is the same convention used by `bio_crates::mr::linalg::wlm`, extended
//! here to a multi-predictor design matrix (no intercept option, since MVMR
//! always regresses through the origin).

use faer::linalg::solvers::{DenseSolveCore, Llt, Solve};
use faer::{Mat, Side};

use crate::error::{MvmrError, Result};

/// Summary of a weighted linear regression through the origin, matching the
/// fields of `summary(lm(y ~ -1 + X, weights = w))` that MVMR consumes.
#[derive(Debug, Clone)]
pub struct WlsSummary {
    /// Coefficients `[β₁,…,β_p]`.
    pub coef: Vec<f64>,
    /// Standard error of each coefficient.
    pub se: Vec<f64>,
    /// Two-sided p-value from the Student-t distribution on `df_resid`.
    pub pvalue: Vec<f64>,
    /// `t_j = β_j / se_j`.
    pub t_stat: Vec<f64>,
    /// Residual standard error `sigma`.
    pub sigma: f64,
    /// Residual degrees of freedom `n − p`.
    pub df_resid: f64,
    /// Per-row weighted residuals `rᵢ = yᵢ − x̂ᵢ`.
    pub residuals: Vec<f64>,
    /// F-statistic (`summary.lm`'s `fstatistic`) for the no-intercept model:
    /// `((Σwᵢ ȳ² − Σwᵢ x̂ᵢ²) / p) / sigma²` where for a through-origin model
    /// R uses the un-centred total sum of squares. Equivalent to
    /// `((TSS − RSS) / p) / (RSS / (n − p))` with `TSS = Σ wᵢ yᵢ²`.
    pub fstatistic: f64,
    /// Numerator df of the F-statistic (= `p`).
    pub f_df_num: f64,
    /// Denominator df of the F-statistic (= `n − p`).
    pub f_df_den: f64,
}

impl WlsSummary {
    /// Degrees of freedom for the two-sided Student-t tail.
    pub fn df(&self) -> f64 {
        self.df_resid
    }
}

/// Fit `lm(y ~ -1 + X1 + ... + Xp, weights = w)` (no intercept).
///
/// * `x` — `n × p` design matrix, row-major (`x[i]` is row `i`).
/// * `y`, `w` — length-`n` response and precision-weight vectors.
///
/// Returns [`MvmrError::Numerical`] on a rank-deficient design (matching R's
/// `NA` coefficient surfacing).
pub fn wls_origin(x: &[Vec<f64>], y: &[f64], w: &[f64]) -> Result<WlsSummary> {
    let n = y.len();
    if n == 0 || x.len() != n || w.len() != n {
        return Err(MvmrError::LengthMismatch(format!(
            "wls_origin: n={} but x rows={}, w len={}",
            n,
            x.len(),
            w.len()
        )));
    }
    let p = x.first().map(|r| r.len()).unwrap_or(0);
    if p == 0 {
        return Err(MvmrError::InsufficientSnps("wls_origin: p == 0".into()));
    }
    for (i, row) in x.iter().enumerate() {
        if row.len() != p {
            return Err(MvmrError::LengthMismatch(format!(
                "wls_origin: row {i} has {} cols, expected {p}",
                row.len()
            )));
        }
    }
    if n <= p {
        return Err(MvmrError::InsufficientSnps(format!(
            "wls_origin: n={n} <= p={p}"
        )));
    }

    // ── Build Xᵀ W X (p×p) and Xᵀ W y (length p), column-major for faer ──
    let mut xtwx = vec![0.0; p * p];
    let mut xtwy = vec![0.0; p];
    for i in 0..n {
        let wi = w[i];
        let yi = y[i];
        for a in 0..p {
            let xa = x[i][a];
            xtwy[a] += wi * xa * yi;
            for b in a..p {
                xtwx[a * p + b] += wi * xa * x[i][b];
            }
        }
    }
    // Mirror the upper triangle into the lower.
    for a in 0..p {
        for b in (a + 1)..p {
            xtwx[b * p + a] = xtwx[a * p + b];
        }
    }

    let gram = Mat::from_fn(p, p, |i, j| xtwx[j * p + i]);
    let llt = Llt::new(gram.as_ref(), Side::Lower).map_err(|e| {
        MvmrError::Numerical(format!("wls_origin: singular design (LLᵀ failed): {e:?}"))
    })?;
    let rhs = Mat::from_fn(p, 1, |i, _| xtwy[i]);
    let sol = llt.solve(&rhs);
    let coef: Vec<f64> = (0..p).map(|i| sol[(i, 0)]).collect();

    // ── Residuals, weighted RSS, sigma ──
    let mut residuals = vec![0.0; n];
    let mut wrss = 0.0;
    for i in 0..n {
        let mut fit = 0.0;
        for a in 0..p {
            fit += coef[a] * x[i][a];
        }
        let r = y[i] - fit;
        residuals[i] = r;
        wrss += w[i] * r * r;
    }
    let df_resid = (n - p) as f64;
    let sigma2 = wrss / df_resid;
    let sigma = sigma2.sqrt();

    // ── Coefficient covariance = sigma² · (Xᵀ W X)⁻¹ ──
    let inv = llt.inverse();
    let se: Vec<f64> = (0..p)
        .map(|j| (sigma2 * inv[(j, j)]).max(0.0).sqrt())
        .collect();

    // ── t-statistics and two-sided p-values (Student-t on df_resid) ──
    let mut t_stat = vec![0.0; p];
    let mut pvalue = vec![0.0; p];
    let df_t = df_resid;
    for j in 0..p {
        let t = coef[j] / se[j];
        t_stat[j] = t;
        pvalue[j] = two_sided_t(t, df_t);
    }

    // ── F-statistic for the no-intercept model ──
    // R's summary.lm computes:
    //   mss <- sum(w * fitted^2)        # model sum of squares
    //   rss <- sum(w * residuals^2)
    //   fstat <- (mss/p) / (rss/(n-p))
    let mut mss = 0.0;
    for i in 0..n {
        let mut fit = 0.0;
        for a in 0..p {
            fit += coef[a] * x[i][a];
        }
        mss += w[i] * fit * fit;
    }
    let fstatistic = (mss / p as f64) / (wrss / df_resid);

    Ok(WlsSummary {
        coef,
        se,
        pvalue,
        t_stat,
        sigma,
        df_resid,
        residuals,
        fstatistic,
        f_df_num: p as f64,
        f_df_den: df_resid,
    })
}

/// Two-sided tail of the Student-t distribution with `df` degrees of freedom.
pub fn two_sided_t(t: f64, df: f64) -> f64 {
    use statrs::distribution::{ContinuousCDF, StudentsT};
    if !t.is_finite() || df <= 0.0 {
        return f64::NAN;
    }
    let dist = match StudentsT::new(0.0, 1.0, df) {
        Ok(d) => d,
        Err(_) => return f64::NAN,
    };
    2.0 * dist.cdf(-t.abs())
}

/// Upper tail of the chi-square distribution with `df` degrees of freedom
/// (`stats::pchisq(q, df, lower.tail = FALSE)`).
pub fn pchisq_upper(q: f64, df: f64) -> f64 {
    use statrs::distribution::{ChiSquared, ContinuousCDF};
    if q < 0.0 || df <= 0.0 {
        return f64::NAN;
    }
    let dist = match ChiSquared::new(df) {
        Ok(d) => d,
        Err(_) => return f64::NAN,
    };
    1.0 - dist.cdf(q)
}

/// Upper tail of the F distribution (`stats::pf(q, df1, df2, lower.tail=FALSE)`).
pub fn pf_upper(q: f64, df1: f64, df2: f64) -> f64 {
    use statrs::distribution::{ContinuousCDF, FisherSnedecor};
    if q < 0.0 || df1 <= 0.0 || df2 <= 0.0 {
        return f64::NAN;
    }
    let dist = match FisherSnedecor::new(df1, df2) {
        Ok(d) => d,
        Err(_) => return f64::NAN,
    };
    1.0 - dist.cdf(q)
}

/// Unweighted OLS through the origin, used for the δ regressions inside
/// `strength_mvmr`. Matches R's `lm(y ~ -1 + X)` (no weights).
pub fn ols_origin(x: &[Vec<f64>], y: &[f64]) -> Result<Vec<f64>> {
    let n = y.len();
    let w = vec![1.0; n];
    let s = wls_origin(x, y, &w)?;
    Ok(s.coef)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64, tol: f64) -> bool {
        (a - b).abs() <= tol * b.abs().max(1e-12)
    }

    #[test]
    fn single_predictor_matches_wlm() {
        // x=[1,2,3], y=[2,4,6], w=[1,1,1] through origin → slope 2, sigma 0.
        let x = vec![vec![1.0], vec![2.0], vec![3.0]];
        let s = wls_origin(&x, &[2.0, 4.0, 6.0], &[1.0; 3]).unwrap();
        assert!(close(s.coef[0], 2.0, 1e-12));
        assert!(s.sigma.abs() < 1e-10);
        assert_eq!(s.df_resid, 2.0);
    }

    #[test]
    fn two_predictor_perfect_fit() {
        // y = 3*x1 + 2*x2 exactly through the origin.
        let x = vec![
            vec![1.0, 1.0],
            vec![2.0, 0.5],
            vec![0.5, 2.5],
            vec![1.5, 1.0],
        ];
        let y: Vec<f64> = x.iter().map(|r| 3.0 * r[0] + 2.0 * r[1]).collect();
        let s = wls_origin(&x, &y, &[1.0; 4]).unwrap();
        assert!(close(s.coef[0], 3.0, 1e-10));
        assert!(close(s.coef[1], 2.0, 1e-10));
        assert!(s.sigma.abs() < 1e-10);
    }

    #[test]
    fn singular_design_errors() {
        // Two identical predictors.
        let x = vec![vec![1.0, 1.0], vec![2.0, 2.0], vec![3.0, 3.0]];
        let r = wls_origin(&x, &[1.0, 2.0, 3.0], &[1.0; 3]);
        assert!(r.is_err());
    }
}
