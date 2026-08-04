//! `summary.crr` — coefficient table, confidence intervals and the likelihood
//! ratio test.
//!
//! Port of `reference/cmprsk/R/cmprsk.R:221-252`.

use statrs::distribution::{ContinuousCDF, Normal};

use crate::crr::CrrFit;

/// One row of the coefficient table.
#[derive(Debug, Clone)]
pub struct CoefRow {
    /// Term label.
    pub term: String,
    /// `β̂`.
    pub coef: f64,
    /// `exp(β̂)` — the subdistribution hazard ratio.
    pub exp_coef: f64,
    /// `SE(β̂)` from the robust variance.
    pub se: f64,
    /// `β̂ / SE`.
    pub z: f64,
    /// Two-sided Wald p-value.
    pub p_value: f64,
    /// `exp(-β̂)`.
    pub exp_neg_coef: f64,
    /// Lower confidence limit for `exp(β̂)`.
    pub ci_lower: f64,
    /// Upper confidence limit for `exp(β̂)`.
    pub ci_upper: f64,
}

/// The `summary.crr` object.
#[derive(Debug, Clone)]
pub struct CrrSummary {
    /// One row per coefficient.
    pub coefficients: Vec<CoefRow>,
    /// Log pseudo-likelihood at `coef`.
    pub loglik: f64,
    /// Log pseudo-likelihood at `β = 0`.
    pub loglik_null: f64,
    /// `-2 · (loglik_null − loglik)`.
    pub logtest: f64,
    /// Degrees of freedom for the likelihood ratio test.
    pub df: usize,
    /// p-value of the likelihood ratio test (χ² with `df` degrees of freedom).
    ///
    /// R's `print.summary.crr` reports the statistic without a p-value; it is
    /// included here because the DAG node surfaces it.
    pub logtest_p: f64,
    /// Whether the fit converged.
    pub converged: bool,
    /// Observations used.
    pub n: usize,
    /// Observations dropped for missing values.
    pub n_missing: usize,
}

/// Build the summary at the requested confidence level (`conf_int = 0.95`
/// reproduces R's default).
pub fn summary_crr(fit: &CrrFit, conf_int: f64) -> CrrSummary {
    let normal = Normal::new(0.0, 1.0).expect("standard normal");
    let a = (1.0 - conf_int) / 2.0;
    let z_lo = normal.inverse_cdf(a);
    let z_hi = normal.inverse_cdf(1.0 - a);

    let coefficients = fit
        .coef
        .iter()
        .enumerate()
        .map(|(i, &beta)| {
            let se = fit.var[i][i].sqrt();
            let z = beta / se;
            let p = 2.0 * (1.0 - normal.cdf(z.abs()));
            CoefRow {
                term: fit.terms[i].clone(),
                coef: beta,
                exp_coef: beta.exp(),
                se,
                z,
                p_value: p,
                exp_neg_coef: (-beta).exp(),
                ci_lower: (beta + z_lo * se).exp(),
                ci_upper: (beta + z_hi * se).exp(),
            }
        })
        .collect();

    let df = fit.coef.len();
    let logtest = -2.0 * (fit.loglik_null - fit.loglik);
    let logtest_p = chisq_upper_tail(logtest, df as f64);

    CrrSummary {
        coefficients,
        loglik: fit.loglik,
        loglik_null: fit.loglik_null,
        logtest,
        df,
        logtest_p,
        converged: fit.converged,
        n: fit.n,
        n_missing: fit.n_missing,
    }
}

/// Upper-tail probability of a χ²(df) variate — `1 - pchisq(q, df)`.
pub(crate) fn chisq_upper_tail(q: f64, df: f64) -> f64 {
    use statrs::distribution::ChiSquared;
    if !q.is_finite() || q <= 0.0 || df <= 0.0 {
        return if q <= 0.0 && df > 0.0 { 1.0 } else { f64::NAN };
    }
    match ChiSquared::new(df) {
        Ok(d) => (1.0 - d.cdf(q)).clamp(0.0, 1.0),
        Err(_) => f64::NAN,
    }
}
