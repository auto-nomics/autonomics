//! E-values for unmeasured confounding.
//!
//! Port of `R/EValue.R`. Computes E-values for risk ratios, odds ratios,
//! hazard ratios, mean differences, OLS estimates, and risk differences.

use crate::error::{EvalueError, Result};
use crate::math_utils::threshold;
use crate::measure::{Estimate, MeasureKind};
use statrs::distribution::ContinuousCDF;

/// Result of an E-value computation.
#[derive(Clone, Debug)]
pub struct EvalueResult {
    /// The effect measure values on the RR scale: (point, lower, upper).
    pub rr_values: [Option<f64>; 3],
    /// The E-values: (point, lower, upper).
    pub evalues: [Option<f64>; 3],
    /// The measure label.
    pub measure: &'static str,
}

impl EvalueResult {
    /// The E-value for the point estimate (summary E-value).
    pub fn point_evalue(&self) -> Option<f64> {
        self.evalues[0]
    }
}

// ── evalues.RR ────────────────────────────────────────────────────────

/// Compute E-value for a risk ratio.
///
/// Port of `evalues.RR()` in `EValue.R:301`.
///
/// - `est`: point estimate
/// - `lo`: lower CI limit (optional)
/// - `hi`: upper CI limit (optional)
/// - `true_val`: true RR to shift to (default 1.0)
pub fn evalues_rr(est: f64, lo: Option<f64>, hi: Option<f64>, true_val: f64) -> Result<EvalueResult> {
    if est < 0.0 {
        return Err(EvalueError::Invalid("RR cannot be negative".into()));
    }
    if true_val < 0.0 {
        return Err(EvalueError::Invalid("True value is impossible".into()));
    }
    if let (Some(lo_v), Some(hi_v)) = (lo, hi) {
        if lo_v > hi_v {
            return Err(EvalueError::Invalid(
                "Lower confidence limit should be less than upper confidence limit".into(),
            ));
        }
    }
    if let Some(lo_v) = lo {
        if est < lo_v {
            return Err(EvalueError::Invalid(
                "Point estimate should be inside confidence interval".into(),
            ));
        }
    }
    if let Some(hi_v) = hi {
        if est > hi_v {
            return Err(EvalueError::Invalid(
                "Point estimate should be inside confidence interval".into(),
            ));
        }
    }

    let values = [Some(est), lo, hi];

    // Compute E-values
    let mut e = values.map(|v| v.and_then(|x| threshold(x, true_val)));

    // Check if CI crosses null
    let null_ci = if est > true_val {
        lo.map(|l| l < true_val)
    } else if est < true_val {
        hi.map(|h| h > true_val)
    } else {
        None
    };

    if let Some(true) = null_ci {
        e[1] = Some(1.0);
        e[2] = Some(1.0);
    }

    // Only report E-value for CI limit closer to null
    if lo.is_some() || hi.is_some() {
        if est > true_val {
            e[2] = None;
        } else if est < true_val {
            e[1] = None;
        } else {
            e[1] = Some(1.0);
            e[2] = None;
        }
    }

    Ok(EvalueResult {
        rr_values: values,
        evalues: e,
        measure: "RR",
    })
}

// ── evalues.OR ────────────────────────────────────────────────────────

/// Compute E-value for an odds ratio.
///
/// Port of `evalues.OR()` in `EValue.R:253`.
pub fn evalues_or(
    est: f64,
    lo: Option<f64>,
    hi: Option<f64>,
    rare: bool,
    true_val: f64,
) -> Result<EvalueResult> {
    if est < 0.0 {
        return Err(EvalueError::Invalid("OR cannot be negative".into()));
    }

    let est_rr = Estimate::or(est, rare).to_rr()?.est;
    let lo_rr = if let Some(l) = lo {
        Some(Estimate::or(l, rare).to_rr()?.est)
    } else {
        None
    };
    let hi_rr = if let Some(h) = hi {
        Some(Estimate::or(h, rare).to_rr()?.est)
    } else {
        None
    };
    let true_rr = Estimate::or(true_val, rare).to_rr()?.est;

    let mut res = evalues_rr(est_rr, lo_rr, hi_rr, true_rr)?;
    res.measure = "OR";
    Ok(res)
}

// ── evalues.HR ────────────────────────────────────────────────────────

/// Compute E-value for a hazard ratio.
///
/// Port of `evalues.HR()` in `EValue.R:182`.
pub fn evalues_hr(
    est: f64,
    lo: Option<f64>,
    hi: Option<f64>,
    rare: bool,
    true_val: f64,
) -> Result<EvalueResult> {
    if est < 0.0 {
        return Err(EvalueError::Invalid("HR cannot be negative".into()));
    }

    let est_rr = Estimate::hr(est, rare).to_rr()?.est;
    let lo_rr = if let Some(l) = lo {
        Some(Estimate::hr(l, rare).to_rr()?.est)
    } else {
        None
    };
    let hi_rr = if let Some(h) = hi {
        Some(Estimate::hr(h, rare).to_rr()?.est)
    } else {
        None
    };
    let true_rr = Estimate::hr(true_val, rare).to_rr()?.est;

    let mut res = evalues_rr(est_rr, lo_rr, hi_rr, true_rr)?;
    res.measure = "HR";
    Ok(res)
}

// ── evalues.MD ────────────────────────────────────────────────────────

/// Compute E-value for a standardized mean difference (Cohen's d).
///
/// Port of `evalues.MD()` in `EValue.R:132`.
pub fn evalues_md(est: f64, se: Option<f64>, true_val: f64) -> Result<EvalueResult> {
    if let Some(se_v) = se {
        if se_v < 0.0 {
            return Err(EvalueError::Invalid("Standard error cannot be negative".into()));
        }
    }

    let lo = se.map(|s| (0.91_f64 * est - 1.78_f64 * s).exp());
    let hi = se.map(|s| (0.91_f64 * est + 1.78_f64 * s).exp());

    let est_rr = Estimate::md(est).to_rr()?.est;
    let true_rr = Estimate::md(true_val).to_rr()?.est;

    let mut res = evalues_rr(est_rr, lo, hi, true_rr)?;
    res.measure = "MD";
    Ok(res)
}

// ── evalues.OLS ───────────────────────────────────────────────────────

/// Compute E-value for a linear regression coefficient.
///
/// Port of `evalues.OLS()` in `EValue.R:79`.
///
/// - `est`: coefficient estimate
/// - `se`: standard error
/// - `sd`: outcome SD
/// - `delta`: exposure contrast (default 1)
/// - `true_val`: true standardized mean difference (default 0)
pub fn evalues_ols(
    est: f64,
    se: Option<f64>,
    sd: f64,
    delta: f64,
    true_val: f64,
) -> Result<EvalueResult> {
    if let Some(se_v) = se {
        if se_v < 0.0 {
            return Err(EvalueError::Invalid("Standard error cannot be negative".into()));
        }
    }

    let mut delta = delta;
    if delta < 0.0 {
        delta = -delta;
    }

    // Convert to MD scale
    let md_est = est * delta / sd;
    let md_se = se.map(|s| s * delta / sd);

    // Now compute as MD
    evalues_md(md_est, md_se, true_val)
}

// ── twoXtwoRR ─────────────────────────────────────────────────────────

/// Estimate risk ratio and CI from a 2×2 table.
///
/// Port of `twoXtwoRR()` in `EValue.R:389`.
///
/// Returns (point, lower, upper).
pub fn two_by_two_rr(n11: f64, n10: f64, n01: f64, n00: f64, alpha: f64) -> (f64, f64, f64) {
    let p1 = n11 / (n11 + n10);
    let p0 = n01 / (n01 + n00);
    let rr = p1 / p0;
    let log_rr = rr.ln();

    let se_log_rr = (1.0 / n11 - 1.0 / (n11 + n10) + 1.0 / n01 - 1.0 / (n01 + n00)).sqrt();
    let z = statrs::distribution::Normal::new(0.0, 1.0).unwrap();
    let q_alpha = z.inverse_cdf(1.0 - alpha / 2.0);

    let upper = (log_rr + q_alpha * se_log_rr).exp();
    let lower = (log_rr - q_alpha * se_log_rr).exp();

    (rr, lower, upper)
}

// ── evalues.RD ────────────────────────────────────────────────────────

/// E-value for a population-standardized risk difference.
///
/// Port of `evalues.RD()` in `EValue.R:505`.
///
/// Returns (est_evalue, lower_evalue).
pub fn evalues_rd(
    n11: f64,
    n10: f64,
    n01: f64,
    n00: f64,
    true_val: f64,
    alpha: f64,
    grid: f64,
) -> Result<(f64, f64)> {
    for v in [n11, n10, n01, n00] {
        if v < 0.0 {
            return Err(EvalueError::Invalid(
                "Negative cell counts are impossible".into(),
            ));
        }
    }

    let n = n10 + n11 + n01 + n00;
    let n1 = n10 + n11; // total X=1
    let n0 = n00 + n01; // total X=0
    let f = n1 / n;

    let p1 = n11 / n1;
    let p0 = n01 / n0;

    if p1 < p0 {
        return Err(EvalueError::Invalid(
            "RD < 0; please relabel the exposure so RD > 0".into(),
        ));
    }
    if p1 - p0 <= true_val {
        return Err(EvalueError::Invalid(
            "For risk difference, true value must be <= point estimate".into(),
        ));
    }

    // Standard errors
    let se_p1 = (p1 * (1.0 - p1) / n1).sqrt();
    let se_p0 = (p0 * (1.0 - p0) / n0).sqrt();

    let s2_f = f * (1.0 - f) / n;
    let s2_p1 = se_p1 * se_p1;
    let s2_p0 = se_p0 * se_p0;
    let diff = p0 * (1.0 - f) - p1 * f;

    // Bias factor and E-value for point estimate
    let est_bf = (((true_val + diff).powi(2) + 4.0 * p1 * p0 * f * (1.0 - f)).sqrt()
        - (true_val + diff))
        / (2.0 * p0 * f);
    let est_evalue = threshold(est_bf, 1.0).unwrap_or(1.0);

    // Lower CI limit
    let z = statrs::distribution::Normal::new(0.0, 1.0).unwrap();
    let z_alpha = z.inverse_cdf(1.0 - alpha / 2.0);
    let lower_ci = p1 - p0 - z_alpha * (s2_p1 + s2_p0).sqrt();

    if lower_ci <= true_val {
        return Ok((est_evalue, 1.0));
    }

    // Grid search for E-value for lower CI limit
    let bf_search: Vec<f64> = {
        let mut v = vec![];
        let mut x = 1.0;
        while x <= est_bf {
            v.push(x);
            x += grid;
        }
        v
    };

    let mut lower_evalue = 1.0;
    for &bf in &bf_search {
        let rd_search = p1 - p0 * bf;
        let f_search = f + (1.0 - f) / bf;
        let low_search = rd_search * f_search
            - z_alpha
                * ((s2_p1 + s2_p0 * bf * bf) * f_search * f_search
                    + rd_search * rd_search * (1.0 - 1.0 / bf).powi(2) * s2_f)
                .sqrt();

        if low_search <= true_val {
            lower_evalue = threshold(bf, 1.0).unwrap_or(1.0);
            break;
        }
    }

    Ok((est_evalue, lower_evalue))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_evalues_rr_leukemia() {
        // VanderWeele & Ding (2017): RR = 0.80, CI [0.71, 0.91]
        let res = evalues_rr(0.80, Some(0.71), Some(0.91), 1.0).unwrap();
        // E-value for point ≈ 1.809
        let pe = res.point_evalue().unwrap();
        assert!((pe - 1.809).abs() < 0.01, "point E-value: {pe}");
        // est < true (0.80 < 1.0), so upper CI limit (0.91) is closer to null
        // R convention: est < true → clear lower, report upper
        let ue = res.evalues[2].unwrap();
        let expected_ue = threshold(0.91, 1.0).unwrap();
        assert!((ue - expected_ue).abs() < 1e-8);
    }

    #[test]
    fn test_evalues_rr_smoking() {
        // Hammond & Horn: RR ≈ 10.73, CI [8.02, 14.36]
        let res = evalues_rr(10.73, Some(8.02), Some(14.36), 1.0).unwrap();
        let pe = res.point_evalue().unwrap();
        assert!((pe - 20.94777).abs() < 0.01, "point E-value: {pe}");
        // est > true, so upper CI E-value is None, lower is reported
        let le = res.evalues[1].unwrap();
        let expected = threshold(8.02, 1.0).unwrap();
        assert!((le - expected).abs() < 0.01);
    }

    #[test]
    fn test_evalues_rr_only_point() {
        let res = evalues_rr(3.5, None, None, 1.0).unwrap();
        let pe = res.point_evalue().unwrap();
        let expected = threshold(3.5, 1.0).unwrap();
        assert!((pe - expected).abs() < 1e-10);
    }

    #[test]
    fn test_evalues_rr_ci_crosses_null() {
        // CI crosses null → CI E-value = 1
        let res = evalues_rr(2.0, Some(0.9), Some(3.0), 1.0).unwrap();
        assert_eq!(res.evalues[1], Some(1.0));
    }

    #[test]
    fn test_evalues_or_nonrare() {
        // OR = 0.86, CI [0.75, 0.99], rare = FALSE
        let res = evalues_or(0.86, Some(0.75), Some(0.99), false, 1.0).unwrap();
        // Convert to RR: sqrt(0.86) ≈ 0.927
        let rr_est = 0.86_f64.sqrt();
        let expected = threshold(rr_est, 1.0).unwrap();
        assert!((res.point_evalue().unwrap() - expected).abs() < 1e-8);
    }

    #[test]
    fn test_evalues_hr_nonrare() {
        // HR = 0.56, CI [0.46, 0.69], rare = FALSE
        let res = evalues_hr(0.56, Some(0.46), Some(0.69), false, 1.0).unwrap();
        let rr_est =
            (1.0 - 0.5_f64.powf(0.56_f64.sqrt())) / (1.0 - 0.5_f64.powf(1.0 / 0.56_f64.sqrt()));
        let expected = threshold(rr_est, 1.0).unwrap();
        assert!((res.point_evalue().unwrap() - expected).abs() < 1e-6);
    }

    #[test]
    fn test_two_by_two_rr() {
        let (rr, lower, upper) = two_by_two_rr(397.0, 78557.0, 51.0, 108778.0, 0.05);
        assert!((rr - 10.729780).abs() < 0.01, "RR: {rr}");
        assert!((lower - 8.017457).abs() < 0.01, "lower: {lower}");
        assert!((upper - 14.359688).abs() < 0.02, "upper: {upper}");
    }

    #[test]
    fn test_evalues_md() {
        // Cohen's d = 0.5, SE = 0.25
        let res = evalues_md(0.5, Some(0.25), 0.0).unwrap();
        let rr_est = (0.91_f64 * 0.5_f64).exp();
        let expected = threshold(rr_est, 1.0).unwrap();
        assert!((res.point_evalue().unwrap() - expected).abs() < 1e-8);
    }

    #[test]
    fn test_evalues_rd_smoking() {
        // Hammond & Horn data
        let (est_e, lo_e) =
            evalues_rd(397.0, 78557.0, 51.0, 108778.0, 0.0, 0.05, 0.0001).unwrap();
        // These should be reasonable positive numbers
        assert!(est_e > 1.0, "est E-value should be > 1: {est_e}");
        assert!(lo_e > 1.0, "lower E-value should be > 1: {lo_e}");
    }

    #[test]
    fn test_evalues_rr_non_null() {
        // Non-null: evalues.RR(est=2, true=1.5)
        let res = evalues_rr(2.0, None, None, 1.5).unwrap();
        let pe = res.point_evalue().unwrap();
        let rat: f64 = 2.0 / 1.5;
        let expected = rat + (rat * (rat - 1.0)).sqrt();
        assert!((pe - expected).abs() < 1e-8);
    }
}
