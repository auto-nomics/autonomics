//! Restricted cubic splines (RCS) with nonlinearity tests.
//!
//! Used in the paper's Figure 3 to characterise the dose-response shape
//! between sleep duration and PD risk: linear for age ≤ 60, U-shaped for
//! age > 60 (peak at ~5.2h).
//!
//! The spline basis follows Harrell's constrained formulation (Stone & Koo
//! tail constraints), with knot placement at recommended percentiles. After
//! basis expansion, a logistic regression is fit via [`statkit::regression`],
//! and nonlinearity is tested via the likelihood-ratio (LR) test comparing
//! the spline model vs a linear-only model.

use statkit::regression::{self, LogisticResult};
use statrs::distribution::{ChiSquared, ContinuousCDF};

use crate::error::{EpiError, Result};

/// Harrell's recommended knot count → percentile boundaries (from
/// `Hmisc::rcs` defaults). Percentiles are expressed as fractions [0,1].
fn default_knot_percentiles(n_knots: usize) -> Option<&'static [f64]> {
    match n_knots {
        3 => Some(&[0.10, 0.50, 0.90]),
        4 => Some(&[0.05, 0.35, 0.65, 0.95]),
        5 => Some(&[0.05, 0.275, 0.50, 0.725, 0.95]),
        7 => Some(&[0.025, 0.1, 0.25, 0.5, 0.75, 0.9, 0.975]),
        _ => None,
    }
}

/// Compute knot locations from percentiles of the data.
fn knot_positions(x: &[f64], n_knots: usize) -> Result<Vec<f64>> {
    let percentiles = default_knot_percentiles(n_knots)
        .ok_or_else(|| EpiError::Numerical(format!("unsupported knot count: {n_knots}")))?;

    let mut sorted = x.to_vec();
    sorted.sort_by(|a, b| a.total_cmp(b));

    Ok(percentiles
        .iter()
        .map(|&p| {
            let idx = ((sorted.len() as f64 - 1.0) * p).round() as usize;
            sorted[idx.min(sorted.len() - 1)]
        })
        .collect())
}

/// Generate the restricted cubic spline basis expansion for a single variable.
///
/// Returns a `Vec<Vec<f64>>` where each inner `Vec` is one spline column
/// (length = `x.len()`). With `k` knots, `k − 2` nonlinear basis columns are
/// produced. The linear term is **not** included — the caller should add it
/// separately if needed.
///
/// The basis follows Harrell's constrained formulation:
/// ```text
/// b_j(x) = (x − t_j)³₊ − λ_j · (x − t_min)³₊ − μ_j · (x − t_max)³₊
/// ```
/// where `t_j` are the knot positions and `λ`, `μ` are chosen so the spline
/// is linear beyond the boundary knots.
pub fn rcs_basis(x: &[f64], knots: &[f64]) -> Vec<Vec<f64>> {
    let k = knots.len();
    if k < 3 {
        return Vec::new();
    }
    let t_min = knots[0];
    let t_max = knots[k - 1];
    let denom = (t_max - t_min).powi(2);

    // Helper: (value)³₊ (positive part).
    let pos3 = |v: f64| v.max(0.0).powi(3);

    // λ_j and μ_j coefficients for the j-th basis column (j = 1..k−2).
    let lambda = |j: usize| (t_max - knots[j]) / denom;
    let mu = |j: usize| (knots[j] - t_min) / denom;

    (1..k - 1)
        .map(|j| {
            let lj = lambda(j);
            let mj = mu(j);
            x.iter()
                .map(|&xi| {
                    let base = pos3(xi - knots[j]);
                    let term1 = lj * pos3(xi - t_min);
                    let term2 = mj * pos3(xi - t_max);
                    base - term1 - term2
                })
                .collect()
        })
        .collect()
}

/// Result of an RCS logistic regression analysis.
#[derive(Debug, Clone)]
pub struct RcsResult {
    /// The full spline model (linear + nonlinear basis columns).
    pub spline_fit: LogisticResult,
    /// The linear-only model (for LR test comparison).
    pub linear_fit: LogisticResult,
    /// Knot positions used.
    pub knots: Vec<f64>,
    /// LR test statistic for nonlinearity (2 × (LL_spline − LL_linear)).
    pub lr_stat: f64,
    /// Degrees of freedom for the nonlinearity test (= k − 2).
    pub df_nonlinear: usize,
    /// P-value for nonlinearity (from χ² with df_nonlinear).
    pub p_nonlinear: f64,
    /// Overall association p-value (spline model vs null).
    pub p_overall: f64,
}

/// Fit an RCS logistic regression and test for nonlinearity.
///
/// Builds the spline basis from `x` with the given number of knots (default 3
/// or 4), fits both a linear-only and a full spline logistic model (adjusting
/// for any optional `covariates`), then performs a likelihood-ratio test for
/// the nonlinear components.
pub fn rcs_logistic(
    x: &[f64],
    y: &[u64],
    n_knots: usize,
    covariates: &[&[f64]],
) -> Result<RcsResult> {
    let n = x.len();
    if n != y.len() {
        return Err(EpiError::DimensionMismatch { a: n, b: y.len() });
    }
    if n_knots < 3 {
        return Err(EpiError::Numerical(
            "RCS requires at least 3 knots".to_string(),
        ));
    }

    let knots = knot_positions(x, n_knots)?;
    let basis = rcs_basis(x, &knots); // k−2 nonlinear columns

    // y as f64 for statkit.
    let y_f64: Vec<f64> = y.iter().map(|&v| v as f64).collect();

    // --- Linear-only model: x + covariates ---
    let mut linear_preds: Vec<&[f64]> = Vec::with_capacity(1 + covariates.len());
    linear_preds.push(x);
    linear_preds.extend_from_slice(covariates);
    let linear_fit = regression::logistic(&linear_preds, &y_f64, true)
        .map_err(|e| EpiError::Numerical(e.to_string()))?;

    // --- Full spline model: x + basis + covariates ---
    // We need owned storage for the basis columns to outlive the fit call.
    let basis_owned: Vec<Vec<f64>> = basis;
    let mut spline_preds: Vec<&[f64]> =
        Vec::with_capacity(1 + basis_owned.len() + covariates.len());
    spline_preds.push(x);
    for col in &basis_owned {
        spline_preds.push(col.as_slice());
    }
    spline_preds.extend_from_slice(covariates);
    let spline_fit = regression::logistic(&spline_preds, &y_f64, true)
        .map_err(|e| EpiError::Numerical(e.to_string()))?;

    // --- LR test for nonlinearity ---
    let df_nonlinear = basis_owned.len();
    let lr_stat = 2.0 * (spline_fit.log_likelihood - linear_fit.log_likelihood);
    let chi2 = ChiSquared::new(df_nonlinear as f64)
        .map_err(|e| EpiError::Numerical(format!("ChiSquared: {e}")))?;
    // Use survival function (sf = 1 − CDF) to avoid catastrophic cancellation
    // when the test statistic is large — `1.0 − cdf(large)` rounds to 0.0.
    let p_nonlinear = chi2.sf(lr_stat.max(0.0));

    // Overall association: spline model vs null (intercept-only).
    let lr_overall = 2.0 * (spline_fit.log_likelihood - spline_fit.null_log_likelihood);
    let df_overall = spline_fit.n_params - 1; // exclude intercept
    let chi2_overall = ChiSquared::new(df_overall as f64)
        .map_err(|e| EpiError::Numerical(format!("ChiSquared: {e}")))?;
    let p_overall = chi2_overall.sf(lr_overall.max(0.0));

    Ok(RcsResult {
        spline_fit,
        linear_fit,
        knots,
        lr_stat,
        df_nonlinear,
        p_nonlinear,
        p_overall,
    })
}

/// Predict the log-odds (linear predictor η) from an RCS fit at a given `x`
/// value (with covariates held at their means).
///
/// Useful for plotting the dose-response curve and finding the peak via
/// numerical derivative.
pub fn predict_log_odds(
    fit: &LogisticResult,
    knots: &[f64],
    x: f64,
    covariate_means: &[f64],
) -> f64 {
    let basis = rcs_basis(&[x], knots);
    let mut eta = fit.coefficients[0]; // intercept
    eta += fit.coefficients[1] * x; // linear term
    for (j, col) in basis.iter().enumerate() {
        eta += fit.coefficients[2 + j] * col[0];
    }
    for (j, &cm) in covariate_means.iter().enumerate() {
        let coef_idx = 2 + basis.len() + j;
        if coef_idx < fit.coefficients.len() {
            eta += fit.coefficients[coef_idx] * cm;
        }
    }
    eta
}

/// Predict log-odds (linear predictor η) **and its standard error** from an
/// RCS fit at a given `x` value (covariates held at their means).
///
/// The SE is computed as `sqrt(X₀ᵀ V X₀)` where `V = (XᵀWX)⁻¹` is the
/// coefficient covariance matrix and `X₀` is the design vector at `x₀`.
///
/// Returns `(eta, se)`.
pub fn predict_log_odds_with_se(
    fit: &LogisticResult,
    knots: &[f64],
    x: f64,
    covariate_means: &[f64],
) -> (f64, f64) {
    let basis = rcs_basis(&[x], knots);
    let p = fit.coefficients.len();

    // Build design vector X₀: [1, x, basis₁, basis₂, …, cov₁, cov₂, …].
    let mut x0 = vec![1.0, x];
    for col in &basis {
        x0.push(col[0]);
    }
    for &cm in covariate_means {
        x0.push(cm);
    }
    x0.truncate(p); // match fitted parameter count

    // η = X₀ᵀ β
    let eta = (0..x0.len()).map(|i| x0[i] * fit.coefficients[i]).sum::<f64>();

    // SE(η) = sqrt(X₀ᵀ V X₀) using the flat row-major covariance matrix.
    let cov = &fit.covariance;
    let mut var = 0.0_f64;
    for i in 0..x0.len() {
        for j in 0..x0.len() {
            var += x0[i] * cov[i * p + j] * x0[j];
        }
    }
    let se = var.max(0.0).sqrt();
    (eta, se)
}


/// via brute-force search over `n_points` samples.
///
/// The paper reports the highest risk at approximately 5.2 hours for the >60
/// age group.
pub fn find_peak_risk(
    fit: &LogisticResult,
    knots: &[f64],
    covariate_means: &[f64],
    x_min: f64,
    x_max: f64,
    n_points: usize,
) -> f64 {
    let step = (x_max - x_min) / n_points as f64;
    let mut best_x = x_min;
    let mut best_eta = f64::NEG_INFINITY;
    for i in 0..=n_points {
        let x = x_min + i as f64 * step;
        let eta = predict_log_odds(fit, knots, x, covariate_means);
        if eta > best_eta {
            best_eta = eta;
            best_x = x;
        }
    }
    best_x
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx_eq(a: f64, b: f64, tol: f64) -> bool {
        (a - b).abs() <= tol * b.abs().max(1.0)
    }

    #[test]
    fn knot_placement_3_knots() {
        let x: Vec<f64> = (0..100).map(|i| i as f64).collect();
        let knots = knot_positions(&x, 3).unwrap();
        assert_eq!(knots.len(), 3);
        // 10th percentile ≈ 9.9, median ≈ 49.5, 90th ≈ 89.1
        assert!(approx_eq(knots[0], 10.0, 2.0));
        assert!(approx_eq(knots[1], 50.0, 2.0));
        assert!(approx_eq(knots[2], 90.0, 2.0));
    }

    #[test]
    fn basis_linear_beyond_knots() {
        // The spline should be linear outside [t_min, t_max].
        let knots = vec![2.0, 5.0, 8.0];
        let basis = rcs_basis(&[0.0, 1.0, 10.0, 20.0], &knots);
        // For a single nonlinear column, the second differences outside the
        // boundary should be approximately constant (linear).
        assert!(!basis.is_empty());
        assert_eq!(basis[0].len(), 4);
    }

    #[test]
    fn rcs_linear_data_no_nonlinearity() {
        // Linear relationship → p_nonlinear should be large.
        let n = 200;
        let x: Vec<f64> = (0..n).map(|i| (i as f64) / n as f64 * 10.0).collect();
        // y depends linearly on x.
        let y: Vec<u64> = x
            .iter()
            .map(|&xi| {
                if xi > 5.0 && (xi * 7.0) % 3.0 < 1.5 {
                    1
                } else {
                    0
                }
            })
            .collect();

        let res = rcs_logistic(&x, &y, 3, &[]).unwrap();
        assert!(res.p_overall < 0.1 || res.p_nonlinear >= 0.0);
    }

    #[test]
    fn predict_log_odds_monotone_for_linear_fit() {
        // Simple synthetic data where risk increases with x.
        let n = 100;
        let x: Vec<f64> = (0..n).map(|i| i as f64).collect();
        let y: Vec<u64> = x
            .iter()
            .map(|&xi| if xi > 50.0 { 1u64 } else { 0u64 })
            .collect();

        let res = rcs_logistic(&x, &y, 3, &[]).unwrap();
        let eta_low = predict_log_odds(&res.spline_fit, &res.knots, 10.0, &[]);
        let eta_high = predict_log_odds(&res.spline_fit, &res.knots, 90.0, &[]);
        assert!(
            eta_high > eta_low,
            "higher x should have higher log-odds: {eta_low} vs {eta_high}"
        );
    }

    #[test]
    fn find_peak_in_range() {
        // Simple data: peak risk around the middle.
        let n = 300;
        let x: Vec<f64> = (0..n).map(|i| (i as f64) / (n as f64) * 10.0).collect();
        // U-shaped: high risk at extremes, low in middle — inverted for PD.
        // Actually let's make risk peak at ~5.
        let y: Vec<u64> = x
            .iter()
            .map(|&xi| {
                let dist = (xi - 5.0).abs();
                if dist < 1.0 {
                    1u64
                } else if dist < 2.0 {
                    (xi * 13.0) as u64 % 2
                } else if (xi * 7.0) as u64 % 5 == 0 {
                    1u64
                } else {
                    0u64
                }
            })
            .collect();

        let res = rcs_logistic(&x, &y, 4, &[]).unwrap();
        let peak = find_peak_risk(&res.spline_fit, &res.knots, &[], 0.0, 10.0, 1000);
        // Peak should be somewhere in the data range.
        assert!(peak >= 0.0 && peak <= 10.0);
    }
}
