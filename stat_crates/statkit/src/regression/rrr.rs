//! Reduced-rank regression (Izenman 1975) via WLS + SVD of the fitted matrix.
//!
//! Fits the multivariate linear model `Y = X B + E` by (weighted) least
//! squares, then restricts the coefficient matrix to rank `k`: with the SVD
//! of the column-centred fitted matrix `Ĥ_c = U D Vᵀ`, the rank-`k` RRR
//! slopes are
//!
//! ```text
//! B_rrr(k) = B_ols · V_k · V_kᵀ ,    intercept = ȳ − x̄ᵀ B_rrr(k)
//! ```
//!
//! (the centred fit is projected and the response mean re-added, so a rank-k
//! fit is lossless whenever the centred fitted matrix has rank ≤ k). The
//! projection minimises the (weighted) Frobenius distance between the fitted
//! responses and any rank-`k` restriction. "Variance explained" is the share
//! of the fitted-response total sum of squares carried by each canonical
//! direction: `d_j² / Σ d²`.
//!
//! This is the exact estimator the t3-013 factor pipeline approximated with
//! a cross-correlation SVD.

use faer::Mat;

use crate::error::{Result, StatError};

/// Result of a reduced-rank regression.
#[derive(Debug, Clone, PartialEq)]
pub struct RrrResult {
    /// Fitted intercept per response (length r) — row 0 of the rank-k B.
    pub intercept: Vec<f64>,
    /// Rank-k coefficient matrix, `coefficients[i][j]` = coefficient of
    /// predictor `i` (excluding the intercept) on response `j`.
    pub coefficients: Vec<Vec<f64>>,
    /// The rank used for the projection.
    pub rank: usize,
    /// Singular values of the centred fitted matrix (descending).
    pub singular_values: Vec<f64>,
    /// Variance explained per canonical direction: `d_j²/Σd²` (descending).
    pub variance_explained: Vec<f64>,
    /// Cumulative sum of `variance_explained`.
    pub cumulative: Vec<f64>,
    /// Weighted R² per response of the rank-k fit (vs the weighted mean).
    pub r_squared_per_response: Vec<f64>,
    /// Number of complete observations used.
    pub n_obs: usize,
}

/// Fit reduced-rank regression of `y` (r response columns) on `x` (p
/// predictor columns) with optional observation `weights`, projecting onto
/// `rank` dimensions (all of `min(p, r)` when `None`).
pub fn reduced_rank_regression(
    x: &[&[f64]],
    y: &[&[f64]],
    weights: Option<&[f64]>,
    rank: Option<usize>,
) -> Result<RrrResult> {
    let n = y.first().map(|c| c.len()).unwrap_or(0);
    if n == 0 || x.is_empty() || y.is_empty() {
        return Err(StatError::EmptyInput);
    }
    let unit: Vec<f64> = vec![1.0; n];
    let w = weights.unwrap_or(&unit);
    if w.len() != n {
        return Err(StatError::LengthMismatch { a: n, b: w.len() });
    }
    if w.iter().any(|&wi| wi < 0.0 || wi.is_nan()) {
        return Err(StatError::InvalidWeights);
    }
    for col in x.iter().chain(y.iter()) {
        if col.len() != n {
            return Err(StatError::LengthMismatch { a: n, b: col.len() });
        }
    }
    let p = x.len();
    let r = y.len();
    let max_rank = p.min(r);
    let rank = match rank {
        Some(0) => return Err(StatError::InsufficientData { min: 1, actual: 0 }),
        Some(k) if k > max_rank => {
            return Err(StatError::InvalidInput(format!(
                "rank {k} exceeds the maximum rank min(p, r) = {max_rank}"
            )));
        }
        Some(k) => k,
        None => max_rank,
    };

    // ── Stage 1: per-response WLS with intercept (the OLS coefficients). ──
    let mut b_ols = Mat::zeros(p + 1, r); // row 0 = intercept
    let mut fitted = Mat::zeros(n, r);
    for (j, ycol) in y.iter().enumerate() {
        let fit = super::wls(x, ycol, w, true)?;
        for i in 0..=p {
            b_ols[(i, j)] = fit.coefficients[i];
        }
        for (k, &f) in fit.fitted.iter().enumerate() {
            fitted[(k, j)] = f;
        }
    }

    // ── Stage 2: centre the fitted matrix by the weighted column means. ──
    let wsum: f64 = w.iter().sum();
    if !(wsum > 0.0) {
        return Err(StatError::InvalidInput("weights sum to zero".into()));
    }
    let mut fitted_c = Mat::zeros(n, r);
    for j in 0..r {
        let wmean = (0..n).map(|k| w[k] * fitted[(k, j)]).sum::<f64>() / wsum;
        for k in 0..n {
            fitted_c[(k, j)] = fitted[(k, j)] - wmean;
        }
    }

    // ── Stage 3: SVD of Ĥ_c and rank-k projection of B. ──
    // Standard RRR: project the centred fit and re-add the response mean, so
    // the intercept is recomputed (ȳ − x̄ᵀ·slopes) rather than projected —
    // a rank-k fit is lossless whenever the centred fitted matrix has
    // rank ≤ k.
    let svd = fitted_c
        .svd()
        .map_err(|e| StatError::Numerical(format!("SVD failed: {e:?}")))?;
    let s_all: Vec<f64> = svd.S().column_vector().iter().copied().collect();
    let v = svd.V();
    let v_k = Mat::from_fn(r, rank, |i, j| v[(i, j)]);
    // Slopes: B_ols rows 1.. (drop the intercept row) · V_k · V_kᵀ  (p × r)
    let b_slopes_ols = b_ols.get(1..b_ols.nrows(), ..);
    let b_slopes = b_slopes_ols * (v_k.clone() * v_k.transpose());

    // Weighted means of the predictors (for the intercept) and responses.
    let x_wmean: Vec<f64> = (0..p)
        .map(|i| (0..n).map(|k| w[k] * x[i][k]).sum::<f64>() / wsum)
        .collect();
    let y_wmean: Vec<f64> = (0..r)
        .map(|j| (0..n).map(|k| w[k] * y[j][k]).sum::<f64>() / wsum)
        .collect();
    let intercept: Vec<f64> = (0..r)
        .map(|j| y_wmean[j] - (0..p).map(|i| x_wmean[i] * b_slopes[(i, j)]).sum::<f64>())
        .collect();

    // Variance explained: d²/Σd² over ALL directions (not truncated at k).
    let total_ss: f64 = s_all.iter().map(|s| s * s).sum();
    let variance_explained: Vec<f64> = s_all
        .iter()
        .map(|&s| {
            if total_ss > 0.0 {
                s * s / total_ss
            } else {
                0.0
            }
        })
        .collect();
    let mut cumulative = Vec::with_capacity(variance_explained.len());
    let mut acc = 0.0;
    for &ve in &variance_explained {
        acc += ve;
        cumulative.push(acc);
    }

    // ── Stage 4: per-response weighted R² of the rank-k fit. ──
    let mut r_squared = Vec::with_capacity(r);
    for j in 0..r {
        let mut sse = 0.0;
        let mut sst = 0.0;
        for k in 0..n {
            let yhat = intercept[j] + (0..p).map(|i| b_slopes[(i, j)] * x[i][k]).sum::<f64>();
            sse += w[k] * (y[j][k] - yhat) * (y[j][k] - yhat);
            sst += w[k] * (y[j][k] - y_wmean[j]) * (y[j][k] - y_wmean[j]);
        }
        r_squared.push(if sst > 0.0 { 1.0 - sse / sst } else { f64::NAN });
    }

    Ok(RrrResult {
        intercept,
        coefficients: (0..p)
            .map(|i| (0..r).map(|j| b_slopes[(i, j)]).collect())
            .collect(),
        rank,
        singular_values: s_all,
        variance_explained,
        cumulative,
        r_squared_per_response: r_squared,
        n_obs: n,
    })
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: f64, b: f64, tol: f64) -> bool {
        (a - b).abs() <= tol
    }

    /// Y1 = 1 + 2·x1 + x2, Y2 = 3 + 4·x1 + 2·x2 (both responses share the
    /// SAME direction in X-space → the fitted matrix is exactly rank 1).
    #[test]
    fn rank1_recovers_exact_coefficients() {
        let n = 40;
        let x1: Vec<f64> = (0..n).map(|i| (i % 7) as f64).collect();
        let x2: Vec<f64> = (0..n).map(|i| ((i * 3) % 11) as f64).collect();
        let y1: Vec<f64> = x1
            .iter()
            .zip(&x2)
            .map(|(&a, &b)| 1.0 + 2.0 * a + b)
            .collect();
        let y2: Vec<f64> = x1
            .iter()
            .zip(&x2)
            .map(|(&a, &b)| 3.0 + 4.0 * a + 2.0 * b)
            .collect();
        let res = reduced_rank_regression(&[&x1, &x2], &[&y1, &y2], None, Some(1)).unwrap();
        // Rank-1 is lossless here: coefficients equal the OLS fit.
        assert!(approx(res.intercept[0], 1.0, 1e-9), "{:?}", res.intercept);
        assert!(approx(res.intercept[1], 3.0, 1e-9));
        assert!(approx(res.coefficients[0][0], 2.0, 1e-9));
        assert!(approx(res.coefficients[1][0], 1.0, 1e-9));
        assert!(approx(res.coefficients[0][1], 4.0, 1e-9));
        assert!(approx(res.coefficients[1][1], 2.0, 1e-9));
        // Rank-1 explains everything.
        assert!(approx(res.variance_explained[0], 1.0, 1e-9));
        assert!(approx(res.r_squared_per_response[0], 1.0, 1e-9));
        assert!(approx(res.r_squared_per_response[1], 1.0, 1e-9));
    }

    #[test]
    fn full_rank_equals_ols() {
        let n = 30;
        let x1: Vec<f64> = (0..n).map(|i| (i % 5) as f64).collect();
        let x2: Vec<f64> = (0..n).map(|i| ((i * 7) % 13) as f64 / 13.0).collect();
        let y1: Vec<f64> = (0..n)
            .map(|i| 2.0 + 1.5 * x1[i] - 0.5 * x2[i] + ((i % 3) as f64 - 1.0) * 0.1)
            .collect();
        let y2: Vec<f64> = (0..n).map(|i| -1.0 + 0.3 * x1[i] + 2.0 * x2[i]).collect();
        let res = reduced_rank_regression(&[&x1, &x2], &[&y1, &y2], None, None).unwrap();
        assert_eq!(res.rank, 2);
        // Full rank ⇒ projection is the identity ⇒ matches per-response WLS.
        let ols1 = super::super::wls(&[&x1, &x2], &y1, &vec![1.0; n], true).unwrap();
        assert!(approx(res.intercept[0], ols1.coefficients[0], 1e-9));
        assert!(approx(res.coefficients[0][0], ols1.coefficients[1], 1e-9));
        assert!(approx(res.coefficients[1][0], ols1.coefficients[2], 1e-9));
        assert!(approx(res.r_squared_per_response[0], ols1.r_squared, 1e-9));
    }

    #[test]
    fn variance_explained_descending_and_normalised() {
        let n = 50;
        let x1: Vec<f64> = (0..n).map(|i| ((i * 3) % 17) as f64 / 17.0).collect();
        let x2: Vec<f64> = (0..n).map(|i| ((i * 5) % 23) as f64 / 23.0).collect();
        let y1: Vec<f64> = (0..n).map(|i| 1.0 + 2.0 * x1[i] + 0.1 * x2[i]).collect();
        let y2: Vec<f64> = (0..n).map(|i| 1.0 + 2.0 * x1[i] - 0.2 * x2[i]).collect();
        let y3: Vec<f64> = (0..n).map(|i| -3.0 + 0.05 * x2[i]).collect();
        let res = reduced_rank_regression(&[&x1, &x2], &[&y1, &y2, &y3], None, None).unwrap();
        // Thin SVD of an n×3 response matrix yields 3 values; the fitted
        // matrix has rank ≤ p = 2, so the third is numerically zero.
        assert_eq!(res.singular_values.len(), 3);
        assert!(res.variance_explained[0] >= res.variance_explained[1] - 1e-12);
        assert!(approx(
            res.variance_explained.iter().sum::<f64>(),
            1.0,
            1e-12
        ));
        assert!(approx(*res.cumulative.last().unwrap(), 1.0, 1e-12));
        assert!(res.singular_values[0] >= res.singular_values[1] - 1e-12);
        assert!(res.singular_values[2].abs() < 1e-9);
    }

    #[test]
    fn constant_weights_leave_fit_unchanged() {
        let n = 25;
        let x1: Vec<f64> = (0..n).map(|i| i as f64).collect();
        let y1: Vec<f64> = (0..n)
            .map(|i| 3.0 + 0.7 * x1[i] + (i % 4) as f64 * 0.05)
            .collect();
        let y2: Vec<f64> = (0..n).map(|i| -2.0 - 0.4 * x1[i]).collect();
        let a = reduced_rank_regression(&[&x1], &[&y1, &y2], None, Some(1)).unwrap();
        let w = vec![7.5; n];
        let b = reduced_rank_regression(&[&x1], &[&y1, &y2], Some(&w), Some(1)).unwrap();
        for j in 0..2 {
            assert!(
                approx(a.intercept[j], b.intercept[j], 1e-9),
                "intercept {j}"
            );
            assert!(approx(a.coefficients[0][j], b.coefficients[0][j], 1e-9));
        }
    }

    #[test]
    fn invalid_inputs_are_errors() {
        let x = [1.0, 2.0, 3.0];
        let y = [1.0, 2.0, 3.0];
        assert!(reduced_rank_regression(&[&x], &[&y], None, Some(0)).is_err());
        assert!(reduced_rank_regression(&[&x], &[&y], None, Some(5)).is_err());
        let w = [1.0, -1.0, 1.0];
        assert!(matches!(
            reduced_rank_regression(&[&x], &[&y], Some(&w), None),
            Err(StatError::InvalidWeights)
        ));
        let yshort = [1.0, 2.0];
        assert!(matches!(
            reduced_rank_regression(&[&x], &[&yshort], None, None),
            Err(StatError::LengthMismatch { .. })
        ));
    }
}
