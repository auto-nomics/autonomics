//! IVW and MR-Egger estimators backing the MR stage ([`crate::mr_runner`]).
//!
//! Vendored verbatim from the removed `bio_crates/mr` TwoSampleMR port
//! (recoverable from git history), which faithfully reproduced TwoSampleMR's
//! `mr_ivw` / `mr_egger_regression` on top of R-compatible `lm` / p-value
//! helpers. Only the subset MRlap needs is kept; the numeric code is
//! unchanged and stays covered by the unit tests below plus the R
//! cross-validation fixtures in `tests/`.

use faer::linalg::solvers::{DenseSolveCore, Llt, Solve};
use faer::{Mat, Side};
use statrs::distribution::{ChiSquared, ContinuousCDF, Normal, StudentsT};

use crate::error::MrlapError;

// ---------------------------------------------------------------------------
// p-value helpers (R `stats::` semantics; `f64::NAN` for invalid parameters)
// ---------------------------------------------------------------------------

/// `pnorm(q, lower.tail = FALSE)` — standard-normal survival 1 − Φ(q).
fn pnorm_sf(q: f64) -> f64 {
    match Normal::new(0.0, 1.0) {
        Ok(d) => d.sf(q),
        Err(_) => f64::NAN,
    }
}

/// Two-sided normal p-value: `2 * pnorm(-|z|)` ≡ `2 * pnorm(|z|, lower.tail=FALSE)`.
fn pnorm_two_sided(z: f64) -> f64 {
    2.0 * pnorm_sf(z.abs())
}

/// Two-sided t p-value: `2 * pt(|t|, df, lower.tail = FALSE)`.
fn pt_two_sided(t: f64, df: f64) -> f64 {
    match StudentsT::new(0.0, 1.0, df) {
        Ok(d) => 2.0 * d.sf(t.abs()),
        Err(_) => f64::NAN,
    }
}

/// `pchisq(q, df, lower.tail = FALSE)`.
fn pchisq_sf(q: f64, df: f64) -> f64 {
    match ChiSquared::new(df) {
        Ok(d) => d.sf(q),
        Err(_) => f64::NAN,
    }
}

// ---------------------------------------------------------------------------
// Weighted linear regression reproducing R's `lm` + `summary.lm`
// ---------------------------------------------------------------------------

/// Summary of a weighted linear regression, matching `summary.lm`.
#[derive(Debug, Clone)]
pub struct WlmSummary {
    /// Coefficients in R's order: `[intercept?, slope]` — i.e. intercept first
    /// when `intercept = true`, then the slope on `x`.
    pub coef: Vec<f64>,
    /// Standard error of each coefficient (`Std. Error` column).
    pub se: Vec<f64>,
    /// Residual standard error (`sigma`), `sqrt(Σ wᵢ rᵢ² / df_resid)`.
    pub sigma: f64,
    /// Residual degrees of freedom `n − p` (as f64 for downstream arithmetic).
    pub df_resid: f64,
    /// Raw weighted residuals `rᵢ = yᵢ − x̂ᵢ` (length n).
    pub residuals: Vec<f64>,
}

impl WlmSummary {
    /// Slope coefficient (last entry of `coef`).
    pub fn slope(&self) -> f64 {
        *self.coef.last().unwrap()
    }
}

/// Fit `lm(y ~ x, weights = w)` (or through the origin when `intercept` is
/// false, i.e. `lm(y ~ -1 + x, weights = w)`).
///
/// R's `lm(y ~ x, weights = w)` treats `w` as **precision weights** (inverse
/// variances): the coefficient minimises `Σ wᵢ (yᵢ − xᵢ β)²`, the residual
/// standard error is `sigma = sqrt(Σ wᵢ rᵢ² / (n − p))`, and the coefficient
/// covariance is `sigma² (Xᵀ W X)⁻¹`. The weights are **not** normalised —
/// normalising them (as `bio_crates::ldsc::linalg::wls` does) would rescale
/// `sigma` and break the IVW `/min(1, sigma)` correction.
///
/// * `x`, `y`, `w` — length-`n` slices. Weights must be > 0 and finite for the
///   regression to be well-defined (matching R, which silently drops non-finite
///   rows only via `na.action`; callers pre-filter NA rows).
/// * Returns [`MrlapError::Numerical`] when the design is rank-deficient (the
///   `(Xᵀ W X)` Gram matrix is not positive-definite), which R surfaces as a
///   `NA` coefficient row.
fn wlm(x: &[f64], y: &[f64], w: &[f64], intercept: bool) -> Result<WlmSummary, MrlapError> {
    let n = y.len();
    if x.len() != n || w.len() != n {
        return Err(MrlapError::numerical(format!(
            "wlm: lengths x={}, y={}, w={}",
            x.len(),
            n,
            w.len()
        )));
    }
    let p = if intercept { 2 } else { 1 };
    if n < p {
        return Err(MrlapError::numerical(format!("wlm: n={n} < p={p}")));
    }

    // Build Xᵀ W X (p×p, symmetric) and Xᵀ W y (length p), column-major for faer.
    let mut xtwx = vec![0.0; p * p];
    let mut xtwy = vec![0.0; p];
    for i in 0..n {
        let wi = w[i];
        let xi = x[i];
        let yi = y[i];
        // Row of the design matrix for this observation.
        let row: [f64; 2] = if intercept { [1.0, xi] } else { [xi, 0.0] };
        for a in 0..p {
            xtwy[a] += wi * row[a] * yi;
            for b in a..p {
                xtwx[a * p + b] += wi * row[a] * row[b];
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
    let llt = Llt::new(gram.as_ref(), Side::Lower)
        .map_err(|e| MrlapError::numerical(format!("wlm: singular design (LLᵀ failed): {e:?}")))?;

    // β = (Xᵀ W X)⁻¹ Xᵀ W y
    let rhs = Mat::from_fn(p, 1, |i, _| xtwy[i]);
    let beta_mat = llt.solve(&rhs);
    let coef: Vec<f64> = (0..p).map(|i| beta_mat[(i, 0)]).collect();

    // Residuals and weighted RSS.
    let mut residuals = vec![0.0; n];
    let mut wrss = 0.0;
    for i in 0..n {
        let xi = x[i];
        let fit = if intercept {
            coef[0] + coef[1] * xi
        } else {
            coef[0] * xi
        };
        let r = y[i] - fit;
        residuals[i] = r;
        wrss += w[i] * r * r;
    }
    let df_resid = (n - p) as f64;
    let sigma2 = wrss / df_resid;
    let sigma = sigma2.sqrt();

    // Coefficient covariance = sigma² · (Xᵀ W X)⁻¹. Extract its diagonal.
    let inv = llt.inverse();
    let se: Vec<f64> = (0..p)
        .map(|j| (sigma2 * inv[(j, j)]).max(0.0).sqrt())
        .collect();

    Ok(WlmSummary {
        coef,
        se,
        sigma,
        df_resid,
        residuals,
    })
}

// ---------------------------------------------------------------------------
// Estimate type + NA semantics (R `is.na`: NaN only; ±Inf count as valid)
// ---------------------------------------------------------------------------

/// A value is valid (non-NA) iff it is not NaN. `±Inf` count as valid,
/// matching R's `is.na`.
#[inline]
fn is_valid(x: &f64) -> bool {
    !x.is_nan()
}

/// A single MR method's estimate.
#[derive(Debug, Clone, Default)]
pub struct MrEstimate {
    /// Causal-effect point estimate.
    pub b: f64,
    /// Standard error of `b`.
    pub se: f64,
    /// Two-sided p-value for `b`.
    pub pval: f64,
    /// Number of SNPs used.
    pub nsnp: usize,

    /// Cochran's Q heterogeneity statistic (when computed).
    pub q: Option<f64>,
    /// Degrees of freedom for Q.
    pub q_df: Option<f64>,
    /// p-value for Q.
    pub q_pval: Option<f64>,

    /// MR-Egger intercept estimate (when computed).
    pub b_i: Option<f64>,
    /// SE of the intercept.
    pub se_i: Option<f64>,
    /// p-value of the intercept.
    pub pval_i: Option<f64>,
}

impl MrEstimate {
    /// An "all-NA" estimate, matching the null lists R returns when there are
    /// too few valid SNPs (e.g. `list(b=NA, se=NA, pval=NA, nsnp=NA)`).
    fn na() -> Self {
        Self {
            b: f64::NAN,
            se: f64::NAN,
            pval: f64::NAN,
            nsnp: 0,
            q: None,
            q_df: None,
            q_pval: None,
            b_i: None,
            se_i: None,
            pval_i: None,
        }
    }
}

/// Count SNPs valid across all four input vectors, mirroring R's
/// `sum(!is.na(b_exp) & !is.na(b_out) & !is.na(se_exp) & !is.na(se_out))`.
fn count_valid4(b_exp: &[f64], b_out: &[f64], se_exp: &[f64], se_out: &[f64]) -> usize {
    b_exp
        .iter()
        .zip(b_out)
        .zip(se_exp)
        .zip(se_out)
        .filter(|(((be, bo), se), so)| {
            is_valid(be) && is_valid(bo) && is_valid(se) && is_valid(so)
        })
        .count()
}

// ---------------------------------------------------------------------------
// IVW — `R/mr.R:986` (`mr_ivw`), through-origin weighted regression
// ---------------------------------------------------------------------------

/// How the IVW standard error is corrected for (under-)dispersion.
#[derive(Clone, Copy)]
enum SeCorrection {
    /// `mr_ivw`: divide by `min(1, sigma)`.
    MinOneSigma,
}

fn ivw_core(
    b_exp: &[f64],
    b_out: &[f64],
    se_exp: &[f64],
    se_out: &[f64],
    weighted: bool,
    corr: SeCorrection,
) -> MrEstimate {
    let n = b_exp.len();
    if count_valid4(b_exp, b_out, se_exp, se_out) < 2 {
        let mut e = MrEstimate::na();
        e.nsnp = n;
        return e;
    }
    let w: Vec<f64> = if weighted {
        se_out.iter().map(|s| 1.0 / (s * s)).collect()
    } else {
        vec![1.0; n]
    };

    let fit = match wlm(b_exp, b_out, &w, false) {
        Ok(f) => f,
        Err(_) => {
            let mut e = MrEstimate::na();
            e.nsnp = n;
            return e;
        }
    };

    let b = fit.slope();
    let se_raw = fit.se[0];
    let sigma = fit.sigma;
    let se = match corr {
        SeCorrection::MinOneSigma => se_raw / sigma.min(1.0),
    };
    let pval = pnorm_two_sided(b / se);

    let q_df = (n - 1) as f64;
    let q = sigma * sigma * q_df;
    let q_pval = pchisq_sf(q, q_df);

    MrEstimate {
        b,
        se,
        pval,
        nsnp: n,
        q: Some(q),
        q_df: Some(q_df),
        q_pval: Some(q_pval),
        ..Default::default()
    }
}

/// `mr_ivw` — default multiplicative-random-effects IVW (`R/mr.R:986`).
pub fn mr_ivw(b_exp: &[f64], b_out: &[f64], se_exp: &[f64], se_out: &[f64]) -> MrEstimate {
    ivw_core(b_exp, b_out, se_exp, se_out, true, SeCorrection::MinOneSigma)
}

// ---------------------------------------------------------------------------
// MR-Egger — `R/mr.R:567 mr_egger_regression`
// ---------------------------------------------------------------------------

/// R's `sign0`: `sign(x)` but 0 → 1.
fn sign0(x: f64) -> f64 {
    if x == 0.0 { 1.0 } else { x.signum() }
}

/// `mr_egger_regression(b_exp, b_out, se_exp, se_out)`.
///
/// Orients all instruments to positive exposure effect (`sign0(b_exp)`,
/// treating 0 as +1), then fits `lm(b_out ~ b_exp, weights = 1/se_out^2)`.
/// Slope = causal estimate, intercept = pleiotropy. SEs are divided by
/// `min(1, sigma)`; p-values use a t distribution with `n−2` df; Rücker's Q is
/// `sigma^2 · (n−2)`.
///
/// Returns an all-NA estimate when fewer than 3 valid SNPs are present, or when
/// the design is collinear (R: "Collinearities in MR Egger").
pub fn mr_egger_regression(
    b_exp: &[f64],
    b_out: &[f64],
    se_exp: &[f64],
    se_out: &[f64],
) -> MrEstimate {
    let n = b_exp.len();
    if count_valid4(b_exp, b_out, se_exp, se_out) < 3 {
        let mut e = MrEstimate::na();
        e.nsnp = n;
        return e;
    }

    // Orient to positive exposure effect.
    let bx: Vec<f64> = b_exp.iter().map(|&b| b.abs()).collect();
    let by: Vec<f64> = b_out
        .iter()
        .zip(b_exp.iter())
        .map(|(&o, &e)| o * sign0(e))
        .collect();
    let w: Vec<f64> = se_out.iter().map(|s| 1.0 / (s * s)).collect();

    let fit = match wlm(&bx, &by, &w, true) {
        Ok(f) => f,
        Err(_) => {
            // Collinear design — R returns the null list.
            let mut e = MrEstimate::na();
            e.nsnp = n;
            return e;
        }
    };

    let df = (n - 2) as f64;
    let sigma = fit.sigma;
    let b = fit.coef[1]; // slope
    let se = fit.se[1] / sigma.min(1.0);
    let pval = pt_two_sided(b / se, df);
    let b_i = fit.coef[0]; // intercept
    let se_i = fit.se[0] / sigma.min(1.0);
    let pval_i = pt_two_sided(b_i / se_i, df);

    let q = sigma * sigma * df;
    let q_pval = pchisq_sf(q, df);

    MrEstimate {
        b,
        se,
        pval,
        nsnp: n,
        q: Some(q),
        q_df: Some(df),
        q_pval: Some(q_pval),
        b_i: Some(b_i),
        se_i: Some(se_i),
        pval_i: Some(pval_i),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: f64, b: f64, tol: f64) -> bool {
        (a - b).abs() <= tol * b.abs().max(1e-12)
    }

    // Analytical case: through-origin WLS, single predictor.
    // x = [1,2,3], y = [2,4,6], w = [1,1,1] → perfect fit, slope 2, sigma 0.
    #[test]
    fn origin_perfect_fit() {
        let s = wlm(&[1.0, 2.0, 3.0], &[2.0, 4.0, 6.0], &[1.0; 3], false).unwrap();
        assert!(approx(s.slope(), 2.0, 1e-12));
        assert!(s.sigma.abs() < 1e-12);
        assert_eq!(s.df_resid, 2.0);
    }

    // Intercept fit against a known OLS solution.
    // x=[0,1,2,3], y=[1,3,5,7] → y = 1 + 2x exactly.
    #[test]
    fn intercept_perfect_fit() {
        let s = wlm(&[0., 1., 2., 3.], &[1., 3., 5., 7.], &[1.; 4], true).unwrap();
        assert!(approx(s.coef[0], 1.0, 1e-12));
        assert!(approx(s.slope(), 2.0, 1e-12));
        assert!(s.sigma.abs() < 1e-12);
    }

    #[test]
    fn singular_design_errors() {
        // All x equal with intercept → rank-deficient.
        assert!(wlm(&[1.0, 1.0, 1.0], &[1.0, 2.0, 3.0], &[1.0; 3], true).is_err());
    }

    // p-value helpers vs R reference values.
    #[test]
    fn pvalue_basics() {
        assert!(approx(pnorm_two_sided(1.95996398454005), 0.05, 1e-9));
        assert!(approx(pt_two_sided(2.0, 10.0), 0.07338803, 1e-6));
        assert!(approx(pchisq_sf(3.84145882069412, 1.0), 0.05, 1e-8));
    }

    #[test]
    fn ivw_two_snp_closed_form() {
        // Through-origin WLS with 2 SNPs, weights 1/se_out^2.
        // x=[1,2], y=[1,2], se_out=[0.1,0.1] → perfect slope 1.
        let e = mr_ivw(&[1.0, 2.0], &[1.0, 2.0], &[0.1, 0.1], &[0.1, 0.1]);
        assert!(approx(e.b, 1.0, 1e-9));
        assert!(approx(e.q_df.unwrap(), 1.0, 1e-12));
        // sigma=0 (perfect fit) → Q=0, Q_pval=1
        assert!(approx(e.q.unwrap(), 0.0, 1e-12));
    }

    #[test]
    fn too_few_snps_is_na() {
        let e = mr_ivw(&[1.0], &[1.0], &[0.1], &[0.1]);
        assert!(e.b.is_nan());
    }

    #[test]
    fn egger_needs_three_snps() {
        let e = mr_egger_regression(&[1.0, 2.0], &[1.0, 2.0], &[0.1, 0.1], &[0.1, 0.1]);
        assert!(e.b.is_nan());
    }

    #[test]
    fn egger_zero_exposure_treated_as_positive() {
        // sign0(0) = 1, so a zero b_exp is kept as 0 (abs), b_out unchanged.
        assert_eq!(sign0(0.0), 1.0);
        assert_eq!(sign0(-3.0), -1.0);
        assert_eq!(sign0(3.0), 1.0);
    }
}
