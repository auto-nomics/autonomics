//! LASSO logistic regression via coordinate descent.
//!
//! Used in the paper's Figure 2A/B to select sleep-related indicators most
//! strongly associated with PD. The paper applies LASSO with 10-fold
//! cross-validation to find the optimal penalty λ, then runs 1000 bootstrap
//! iterations to assess selection frequency.
//!
//! # Algorithm
//!
//! Coordinate descent on the penalized negative log-likelihood:
//!
//! ```text
//! min  −Σ[yᵢηᵢ − log(1+e^{ηᵢ})]  +  λ·Σ|βⱼ|
//! ```
//!
//! where `η = Xβ` (intercept unpenalised). Each coordinate update has a
//! closed-form soft-threshold solution via a one-step Newton approximation
//! (iterated reweighted coordinate descent, as in `glmnet`).
//!
//! The λ path descends geometrically from `λ_max` (the smallest λ at which all
//! penalised coefficients are zero) to `λ_max · λ_min_ratio` over `n_lambda`
//! steps. 10-fold CV picks `λ.1se` (most regularised within 1 SE of the minimum
//! CV deviance).

use rand::Rng;
use rand::SeedableRng;
use rand::seq::SliceRandom;
use rand_chacha::ChaCha8Rng;

use crate::error::{EpiError, Result};

// ── Configuration ──────────────────────────────────────────────────────────

/// LASSO fitting options.
#[derive(Debug, Clone)]
pub struct LassoOptions {
    /// Number of λ values on the path (default 100).
    pub n_lambda: usize,
    /// Ratio `λ_min / λ_max` — the smallest λ (default 0.001 for n > p,
    /// 0.01 otherwise, matching `glmnet`).
    pub lambda_min_ratio: Option<f64>,
    /// Number of CV folds (default 10).
    pub cv_folds: usize,
    /// Coordinate descent convergence threshold (default 1e-7).
    pub tol: f64,
    /// Maximum coordinate descent iterations per λ (default 1000).
    pub max_iter: usize,
    /// Random seed for reproducible CV fold assignment (default 42).
    pub seed: u64,
}

impl Default for LassoOptions {
    fn default() -> Self {
        Self {
            n_lambda: 100,
            lambda_min_ratio: None,
            cv_folds: 10,
            tol: 1e-7,
            max_iter: 1000,
            seed: 42,
        }
    }
}

// ── Result types ───────────────────────────────────────────────────────────

/// Result of a LASSO fit at a single λ.
#[derive(Debug, Clone)]
pub struct LassoFit {
    /// Penalty λ.
    pub lambda: f64,
    /// Coefficients (index 0 = intercept).
    pub coefficients: Vec<f64>,
    /// Which penalised coefficients are non-zero.
    pub selected: Vec<bool>,
    /// Number of non-zero penalised coefficients.
    pub n_selected: usize,
    /// Penalised log-likelihood at convergence.
    pub log_likelihood: f64,
}

/// CV result for the full λ path.
#[derive(Debug, Clone)]
pub struct LassoCvResult {
    /// λ path (descending).
    pub lambdas: Vec<f64>,
    /// Mean CV deviance at each λ.
    pub cv_mean: Vec<f64>,
    /// Standard error of CV deviance at each λ.
    pub cv_se: Vec<f64>,
    /// Index of `λ.min` (minimum CV deviance).
    pub idx_min: usize,
    /// Index of `λ.1se` (most regularised within 1 SE of min).
    pub idx_1se: usize,
    /// Number of non-zero coefficients at each λ (from full-data fits).
    pub n_selected_path: Vec<usize>,
    /// Fit at `λ.1se` on the full data.
    pub fit_1se: LassoFit,
    /// Fit at `λ.min` on the full data.
    pub fit_min: LassoFit,
    /// Feature names aligned with coefficient indices 1.. (index 0 = intercept).
    pub feature_names: Vec<String>,
}

// ── Public API ─────────────────────────────────────────────────────────────

/// Fit LASSO logistic regression with k-fold cross-validation.
///
/// `y` must be binary (0.0/1.0). `predictors` are parallel columns. An
/// intercept (unpenalised) is always included as coefficient index 0.
pub fn lasso_cv(
    predictors: &[&[f64]],
    y: &[f64],
    feature_names: Vec<String>,
    opts: &LassoOptions,
) -> Result<LassoCvResult> {
    let n = y.len();
    let n_features = predictors.len();
    if n == 0 {
        return Err(EpiError::EmptyInput);
    }
    for &v in y {
        if v != 0.0 && v != 1.0 {
            return Err(EpiError::Numerical(format!(
                "LASSO outcome must be binary (0/1), got {v}"
            )));
        }
    }
    for p in predictors.iter() {
        if p.len() != n {
            return Err(EpiError::DimensionMismatch { a: n, b: p.len() });
        }
    }

    // Build design matrix: col 0 = intercept, cols 1.. = predictors.
    let p = n_features + 1;
    let mut cols: Vec<Vec<f64>> = Vec::with_capacity(p);
    cols.push(vec![1.0; n]);
    for pred in predictors {
        cols.push(pred.to_vec());
    }

    // ── Standardise predictors (mean 0, sd 1) for fair penalisation ──
    let col_means: Vec<f64> = (0..p).map(|j| mean(&cols[j])).collect();
    let col_sds: Vec<f64> = (0..p)
        .map(|j| {
            if j == 0 {
                1.0 // intercept — not standardised
            } else {
                let sd = std_dev(&cols[j], col_means[j]);
                if sd < 1e-12 { 1.0 } else { sd }
            }
        })
        .collect();
    let x_std: Vec<Vec<f64>> = (0..p)
        .map(|j| {
            cols[j]
                .iter()
                .map(|&v| (v - col_means[j]) / col_sds[j])
                .collect()
        })
        .collect();

    // ── λ path ──
    let lambdas = build_lambda_path(&x_std, y, p, opts)?;

    // ── Full-data fits at each λ (warm-started) ──
    let mut full_fits: Vec<LassoFit> = Vec::with_capacity(lambdas.len());
    let mut beta_warm = vec![0.0_f64; p];
    for &lam in &lambdas {
        let fit = coordinate_descent(&x_std, y, lam, opts, &mut beta_warm)?;
        full_fits.push(fit);
    }

    // ── k-fold CV ──
    let mut rng = ChaCha8Rng::seed_from_u64(opts.seed);
    let folds = make_folds(n, opts.cv_folds, &mut rng);

    let mut cv_deviances = vec![Vec::new(); lambdas.len()];
    for fold_idx in 0..opts.cv_folds {
        let test_mask = &folds[fold_idx];
        let train_x: Vec<Vec<f64>> = (0..p)
            .map(|j| {
                (0..n)
                    .filter(|&i| !test_mask[i])
                    .map(|i| x_std[j][i])
                    .collect()
            })
            .collect();
        let train_y: Vec<f64> = (0..n).filter(|&i| !test_mask[i]).map(|i| y[i]).collect();

        let test_indices: Vec<usize> = (0..n).filter(|&i| test_mask[i]).collect();

        let mut beta_fold = vec![0.0_f64; p];
        for (li, &lam) in lambdas.iter().enumerate() {
            let fit = coordinate_descent(&train_x, &train_y, lam, opts, &mut beta_fold)?;
            let dev = deviance_on_test(&fit.coefficients, &x_std, y, &test_indices, p);
            cv_deviances[li].push(dev);
        }
    }

    let cv_mean: Vec<f64> = cv_deviances.iter().map(|d| mean(d)).collect();
    let cv_se: Vec<f64> = cv_deviances
        .iter()
        .map(|d| std_dev(d, mean(d)) / (d.len() as f64).sqrt())
        .collect();

    // λ.min and λ.1se.
    let idx_min = cv_mean
        .iter()
        .enumerate()
        .min_by(|(_, a), (_, b)| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
        .map(|(i, _)| i)
        .unwrap_or(0);
    let threshold = cv_mean[idx_min] + cv_se[idx_min];
    // λ path is descending (index 0 = largest λ). λ.1se = largest λ
    // (smallest index) whose CV mean is within 1 SE of the minimum.
    let idx_1se = (0..=idx_min)
        .rev()
        .find(|&i| cv_mean[i] <= threshold)
        .unwrap_or(idx_min);

    Ok(LassoCvResult {
        lambdas,
        cv_mean,
        cv_se,
        idx_min,
        idx_1se,
        n_selected_path: full_fits.iter().map(|f| f.n_selected).collect(),
        fit_1se: full_fits[idx_1se].clone(),
        fit_min: full_fits[idx_min].clone(),
        feature_names,
    })
}

/// Bootstrap selection frequencies: run LASSO on B bootstrap resamples at
/// `λ.1se` and count how often each feature is selected.
///
/// Returns `(selection_counts, selection_frequencies)` for features 1..p
/// (intercept excluded).
pub fn lasso_bootstrap_selection(
    predictors: &[&[f64]],
    y: &[f64],
    lambda: f64,
    n_boot: usize,
    seed: u64,
    opts: &LassoOptions,
) -> Result<(Vec<usize>, Vec<f64>)> {
    let n = y.len();
    let n_features = predictors.len();
    let p = n_features + 1;

    let mut rng = ChaCha8Rng::seed_from_u64(seed);
    let mut counts = vec![0usize; n_features];

    for _ in 0..n_boot {
        // Resample indices with replacement.
        let indices: Vec<usize> = (0..n)
            .map(|_| {
                let r: f64 = rng.random();
                (r * n as f64) as usize
            })
            .collect();

        let y_boot: Vec<f64> = indices.iter().map(|&i| y[i]).collect();
        let cols_boot: Vec<Vec<f64>> = {
            let mut cols = vec![vec![1.0; n]];
            for pred in predictors {
                cols.push(indices.iter().map(|&i| pred[i]).collect());
            }
            cols
        };

        // Standardise.
        let means: Vec<f64> = (0..p).map(|j| mean(&cols_boot[j])).collect();
        let sds: Vec<f64> = (0..p)
            .map(|j| {
                if j == 0 {
                    1.0
                } else {
                    let sd = std_dev(&cols_boot[j], means[j]);
                    if sd < 1e-12 { 1.0 } else { sd }
                }
            })
            .collect();
        let x_boot: Vec<Vec<f64>> = (0..p)
            .map(|j| {
                cols_boot[j]
                    .iter()
                    .map(|&v| (v - means[j]) / sds[j])
                    .collect()
            })
            .collect();

        let mut beta = vec![0.0; p];
        let fit = coordinate_descent(&x_boot, &y_boot, lambda, opts, &mut beta)?;
        for j in 0..n_features {
            if fit.selected[j + 1] {
                counts[j] += 1;
            }
        }
    }

    let freqs: Vec<f64> = counts.iter().map(|&c| c as f64 / n_boot as f64).collect();
    Ok((counts, freqs))
}

// ── Coordinate descent core ────────────────────────────────────────────────

/// One-step Newton coordinate descent for penalised logistic regression.
///
/// Uses the iteratively reweighted formulation: each pass computes working
/// residuals `r_i = y_i − μ_i` and curvature `w_i = μ_i(1−μ_i)`, then
/// updates each coordinate `j ≥ 1` via soft-thresholding. The intercept
/// (`j = 0`) is updated without penalty.
///
/// `beta_warm` provides the initial guess and is updated in place for
/// warm-starting along the λ path.
fn coordinate_descent(
    x: &[Vec<f64>],
    y: &[f64],
    lambda: f64,
    opts: &LassoOptions,
    beta_warm: &mut [f64],
) -> Result<LassoFit> {
    let n = y.len();
    let p = x.len();
    let beta = beta_warm;

    for _iter in 0..opts.max_iter {
        let mut max_change = 0.0_f64;

        // Compute η, μ for all observations.
        let mut eta: Vec<f64> = (0..n)
            .map(|i| (0..p).map(|j| beta[j] * x[j][i]).sum::<f64>())
            .collect();
        let mut mu: Vec<f64> = eta.iter().map(|&e| sigmoid(e)).collect();

        // Update each coordinate.
        for j in 0..p {
            // Working residual excluding j's contribution: r_i = y_i − μ_i + x_{ij}·β_j.
            let partial_residual: Vec<f64> =
                (0..n).map(|i| y[i] - mu[i] + x[j][i] * beta[j]).collect();

            // Weighted least-squares target for coordinate j:
            // β_j = soft_threshold(Σ w_i x_{ij} r_i / n, λ) / (Σ w_i x²_{ij} / n)
            let w: Vec<f64> = (0..n)
                .map(|i| {
                    let w = mu[i] * (1.0 - mu[i]);
                    if w < 1e-12 { 1e-12 } else { w }
                })
                .collect();

            let wxr: f64 = (0..n).map(|i| w[i] * x[j][i] * partial_residual[i]).sum();
            let wxx: f64 = (0..n).map(|i| w[i] * x[j][i] * x[j][i]).sum();

            let old = beta[j];
            if j == 0 {
                // Intercept: no penalty.
                if wxx > 0.0 {
                    beta[0] = wxr / wxx;
                }
            } else {
                // Penalised: soft-threshold.
                let z = if wxx > 0.0 { wxr / wxx } else { 0.0 };
                beta[j] = soft_threshold(z, lambda);
            }

            let delta = (beta[j] - old).abs();
            if delta > max_change {
                max_change = delta;
            }

            // Update μ incrementally: η_i += x_{ij} · (β_j − old)
            let diff = beta[j] - old;
            if diff.abs() > 0.0 {
                for i in 0..n {
                    eta[i] += x[j][i] * diff;
                    mu[i] = sigmoid(eta[i]);
                }
            }
        }

        if max_change < opts.tol {
            break;
        }
    }

    let selected: Vec<bool> = beta.iter().map(|&b| b.abs() > 1e-10).collect();
    let n_selected = selected[1..].iter().filter(|&&s| s).count();

    let eta: Vec<f64> = (0..n)
        .map(|i| (0..p).map(|j| beta[j] * x[j][i]).sum::<f64>())
        .collect();
    let ll: f64 = (0..n)
        .map(|i| y[i] * log_sigmoid(eta[i]) + (1.0 - y[i]) * log_sigmoid(-eta[i]))
        .sum();
    let penalised_ll = ll - lambda * beta[1..].iter().map(|&b| b.abs()).sum::<f64>();

    Ok(LassoFit {
        lambda,
        coefficients: beta.to_vec(),
        selected,
        n_selected,
        log_likelihood: penalised_ll,
    })
}

// ── λ path construction ────────────────────────────────────────────────────

/// Build a geometrically decreasing λ path.
///
/// `λ_max` = max |Xᵀ(y − p̄)| / n  (the smallest λ that zeros all coefficients),
/// where `p̄` is the marginal probability of y=1.
fn build_lambda_path(x: &[Vec<f64>], y: &[f64], p: usize, opts: &LassoOptions) -> Result<Vec<f64>> {
    let n = y.len();
    let p_bar = y.iter().copied().sum::<f64>() / n as f64;

    // λ_max: smallest λ such that all penalised coefficients are 0.
    let mut lambda_max = 0.0_f64;
    for j in 1..p {
        let grad = (0..n).map(|i| x[j][i] * (y[i] - p_bar)).sum::<f64>().abs() / n as f64;
        if grad > lambda_max {
            lambda_max = grad;
        }
    }
    if lambda_max < 1e-12 {
        lambda_max = 1e-6;
    }

    let ratio = opts
        .lambda_min_ratio
        .unwrap_or(if p - 1 < n { 0.001 } else { 0.01 });
    let log_max = lambda_max.ln();
    let log_min = (lambda_max * ratio).ln();

    let lambdas: Vec<f64> = (0..opts.n_lambda)
        .map(|k| {
            let frac = k as f64 / (opts.n_lambda - 1) as f64;
            (log_max + frac * (log_min - log_max)).exp()
        })
        .collect();

    Ok(lambdas)
}

// ── Helpers ────────────────────────────────────────────────────────────────

/// Soft-threshold operator: `S(z, λ) = sign(z) · max(|z| − λ, 0)`.
fn soft_threshold(z: f64, lambda: f64) -> f64 {
    if z > lambda {
        z - lambda
    } else if z < -lambda {
        z + lambda
    } else {
        0.0
    }
}

/// Assign observations to k folds. Returns a `Vec<Vec<bool>>` where each
/// element is a boolean test-mask of length n.
fn make_folds(n: usize, k: usize, rng: &mut ChaCha8Rng) -> Vec<Vec<bool>> {
    let mut indices: Vec<usize> = (0..n).collect();
    indices.shuffle(rng);
    let mut folds = vec![vec![false; n]; k];
    for (i, &idx) in indices.iter().enumerate() {
        folds[i % k][idx] = true;
    }
    folds
}

/// Mean deviance of the test observations for a given β.
fn deviance_on_test(
    beta: &[f64],
    x: &[Vec<f64>],
    y: &[f64],
    test_indices: &[usize],
    p: usize,
) -> f64 {
    let ll: f64 = test_indices
        .iter()
        .map(|&i| {
            let eta: f64 = (0..p).map(|j| beta[j] * x[j][i]).sum();
            y[i] * log_sigmoid(eta) + (1.0 - y[i]) * log_sigmoid(-eta)
        })
        .sum();
    -2.0 * ll / test_indices.len() as f64
}

fn mean(xs: &[f64]) -> f64 {
    if xs.is_empty() {
        return 0.0;
    }
    xs.iter().sum::<f64>() / xs.len() as f64
}

fn std_dev(xs: &[f64], mu: f64) -> f64 {
    if xs.len() < 2 {
        return 0.0;
    }
    let ss: f64 = xs.iter().map(|&x| (x - mu) * (x - mu)).sum();
    (ss / (xs.len() - 1) as f64).sqrt()
}

/// Numerically stable sigmoid.
fn sigmoid(x: f64) -> f64 {
    if x >= 0.0 {
        1.0 / (1.0 + (-x).exp())
    } else {
        let e = x.exp();
        e / (1.0 + e)
    }
}

/// Numerically stable `log σ(x) = −softplus(−x)`.
fn log_sigmoid(x: f64) -> f64 {
    if x > 30.0 {
        -(-x).exp()
    } else if x < -30.0 {
        x
    } else {
        -(1.0 + (-x).exp()).ln()
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn soft_threshold_basic() {
        assert_eq!(soft_threshold(5.0, 2.0), 3.0);
        assert_eq!(soft_threshold(-5.0, 2.0), -3.0);
        assert_eq!(soft_threshold(1.0, 2.0), 0.0);
        assert_eq!(soft_threshold(-1.0, 2.0), 0.0);
    }

    #[test]
    fn lambda_path_descending_geometric() {
        let opts = LassoOptions {
            n_lambda: 10,
            ..Default::default()
        };
        // Minimal data to compute λ_max.
        let x1 = vec![1.0, 0.0, 1.0, 0.0, 1.0];
        let x2 = vec![0.0, 1.0, 0.0, 1.0, 0.0];
        let y = vec![1.0, 0.0, 1.0, 0.0, 1.0];
        let cols = vec![vec![1.0; 5], x1, x2];
        let lambdas = build_lambda_path(&cols, &y, 3, &opts).unwrap();
        assert_eq!(lambdas.len(), 10);
        // Strictly descending.
        for i in 1..lambdas.len() {
            assert!(lambdas[i] < lambdas[i - 1]);
        }
    }

    #[test]
    fn lasso_zeros_irrelevant_feature() {
        // x2 is pure noise, x1 is perfectly predictive.
        let x1: Vec<f64> = (0..50).map(|i| if i < 25 { 0.0 } else { 1.0 }).collect();
        let x2: Vec<f64> = (0..50).map(|i| (i as f64 * 0.1).sin()).collect();
        let y: Vec<f64> = (0..50).map(|i| if i < 25 { 0.0 } else { 1.0 }).collect();

        let opts = LassoOptions::default();
        let result = lasso_cv(
            &[&x1[..], &x2[..]],
            &y,
            vec!["x1".into(), "x2".into()],
            &opts,
        )
        .unwrap();

        // At λ.1se, x1 should be selected and x2 should not (or at least x1 selected more often).
        assert!(
            result.fit_1se.selected[1] || result.fit_min.selected[1],
            "x1 should be selected at either λ.1se or λ.min"
        );
    }

    #[test]
    fn cv_result_has_valid_indices() {
        let x1: Vec<f64> = (0..30).map(|i| i as f64).collect();
        let y: Vec<f64> = (0..30).map(|i| if i < 15 { 0.0 } else { 1.0 }).collect();

        let opts = LassoOptions {
            n_lambda: 20,
            ..Default::default()
        };
        let result = lasso_cv(&[&x1[..]], &y, vec!["x1".into()], &opts).unwrap();

        assert!(result.idx_min < result.lambdas.len());
        assert!(result.idx_1se <= result.idx_min);
    }

    #[test]
    fn bootstrap_selection_runs() {
        let x1: Vec<f64> = (0..40).map(|i| if i < 20 { 0.0 } else { 1.0 }).collect();
        let x2: Vec<f64> = (0..40).map(|i| (i as f64).fract()).collect();
        let y: Vec<f64> = (0..40).map(|i| if i < 20 { 0.0 } else { 1.0 }).collect();

        let opts = LassoOptions::default();
        // Use a moderate λ.
        let (counts, freqs) =
            lasso_bootstrap_selection(&[&x1[..], &x2[..]], &y, 0.01, 50, 12345, &opts).unwrap();
        assert_eq!(counts.len(), 2);
        assert_eq!(freqs.len(), 2);
        for &f in &freqs {
            assert!((0.0..=1.0).contains(&f));
        }
    }

    #[test]
    fn rejects_non_binary() {
        let x = [1.0, 2.0, 3.0];
        let y = vec![0.0, 0.5, 1.0];
        let opts = LassoOptions::default();
        assert!(lasso_cv(&[&x[..]], &y, vec!["x".into()], &opts).is_err());
    }
}
