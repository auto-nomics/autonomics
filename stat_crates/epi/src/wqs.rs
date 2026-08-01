//! Weighted Quantile Sum (WQS) regression.
//!
//! Used in the paper's Figure 2C to assess the relative contribution of each
//! sleep indicator (selected by LASSO) to PD risk. WQS constructs a single
//! weighted index from multiple exposure variables, then tests whether that
//! index is associated with the outcome in a logistic regression.
//!
//! # Model
//!
//! ```text
//! logit(P(y=1)) = β₀ + β₁ · WQS + Z·γ
//! ```
//!
//! where `WQS = Σⱼ wⱼ · qⱼ(Xⱼ)` with `qⱼ` being the quantile-transformed
//! exposure (0, 1, …, n_quantiles−1). Constraints: `Σwⱼ = 1`, `wⱼ ≥ 0`,
//! and the model is estimated in the **positive direction** (β₁ expected
//! positive — all weights reflect harmful contributions).
//!
//! # Estimation
//!
//! Constrained nonlinear optimisation via projected gradient descent on the
//! simplex. The dataset is split 4:6 (train:test); the WQS index and β₁ are
//! estimated on the training set, then evaluated on the test set. B bootstrap
//! resamples provide weight stability estimates.

use rand::Rng;
use rand::SeedableRng;
use rand::seq::SliceRandom;
use rand_chacha::ChaCha8Rng;

use statkit::regression;

use crate::error::{EpiError, Result};

// ── Configuration ──────────────────────────────────────────────────────────

/// WQS regression options.
#[derive(Debug, Clone)]
pub struct WqsOptions {
    /// Number of quantiles for exposure transformation (default 4 = quartiles).
    pub n_quantiles: usize,
    /// Fraction for the training set (default 0.4 = 40% train, 60% test).
    pub train_frac: f64,
    /// Number of bootstrap iterations (default 1000).
    pub n_bootstrap: usize,
    /// Direction: `true` = positive (test for harmful effect), `false` = negative.
    pub positive: bool,
    /// Projected gradient descent learning rate.
    pub learning_rate: f64,
    /// Maximum PGD iterations.
    pub max_iter: usize,
    /// Convergence tolerance on weight change.
    pub tol: f64,
    /// Random seed.
    pub seed: u64,
}

impl Default for WqsOptions {
    fn default() -> Self {
        Self {
            n_quantiles: 4,
            train_frac: 0.4,
            n_bootstrap: 1000,
            positive: true,
            learning_rate: 0.01,
            max_iter: 1000,
            tol: 1e-6,
            seed: 42,
        }
    }
}

// ── Result types ───────────────────────────────────────────────────────────

/// Result of a WQS regression analysis.
#[derive(Debug, Clone)]
pub struct WqsResult {
    /// Feature names for the exposure variables.
    pub feature_names: Vec<String>,
    /// Final estimated weights (normalised to sum 1).
    pub weights: Vec<f64>,
    /// Mean bootstrap weights.
    pub bootstrap_mean_weights: Vec<f64>,
    /// Standard error of bootstrap weights.
    pub bootstrap_se_weights: Vec<f64>,
    /// WQS index coefficient β₁ from the final logistic regression.
    pub beta_wqs: f64,
    /// Standard error of β₁.
    pub beta_wqs_se: f64,
    /// p-value for β₁ (Wald z-test).
    pub beta_wqs_p: f64,
    /// Odds ratio for the WQS index.
    pub wqs_or: f64,
    /// 95% CI lower bound for the WQS OR.
    pub wqs_or_lower: f64,
    /// 95% CI upper bound for the WQS OR.
    pub wqs_or_upper: f64,
    /// p-value from the test-set validation logistic regression.
    pub test_p_value: f64,
    /// Number of observations (training set).
    pub n_train: usize,
    /// Number of observations (test set).
    pub n_test: usize,
}

// ── Public API ─────────────────────────────────────────────────────────────

/// Fit a Weighted Quantile Sum (WQS) regression.
///
/// `exposures` are the exposure variables (parallel columns). `y` is binary
/// (0/1). Optional `covariates` are included as unweighted regressors Z in the
/// logistic model.
pub fn wqs(
    exposures: &[&[f64]],
    y: &[f64],
    covariates: &[&[f64]],
    feature_names: Vec<String>,
    opts: &WqsOptions,
) -> Result<WqsResult> {
    let n = y.len();
    let k = exposures.len();
    if n == 0 || k == 0 {
        return Err(EpiError::EmptyInput);
    }
    for &v in y {
        if v != 0.0 && v != 1.0 {
            return Err(EpiError::Numerical(format!(
                "WQS outcome must be binary (0/1), got {v}"
            )));
        }
    }
    for e in exposures.iter() {
        if e.len() != n {
            return Err(EpiError::DimensionMismatch { a: n, b: e.len() });
        }
    }
    for c in covariates.iter() {
        if c.len() != n {
            return Err(EpiError::DimensionMismatch { a: n, b: c.len() });
        }
    }

    // ── Quantile-transform exposures ──
    let q_exposures: Vec<Vec<f64>> = exposures
        .iter()
        .map(|e| quantile_transform(e, opts.n_quantiles))
        .collect();

    // ── Train/test split ──
    let mut rng = ChaCha8Rng::seed_from_u64(opts.seed);
    let mut indices: Vec<usize> = (0..n).collect();
    indices.shuffle(&mut rng);
    let n_train = (n as f64 * opts.train_frac).round() as usize;
    let train_idx = &indices[..n_train];
    let test_idx = &indices[n_train..];

    // ── Optimise weights on training set ──
    let weights = optimise_weights(&q_exposures, y, covariates, train_idx, opts)?;

    // ── Bootstrap weight stability ──
    let (boot_mean, boot_se) = bootstrap_weights(&q_exposures, y, covariates, train_idx, opts)?;

    // ── Final logistic regression with WQS index ──
    // Build WQS index for all observations.
    let wqs_index: Vec<f64> = (0..n)
        .map(|i| (0..k).map(|j| weights[j] * q_exposures[j][i]).sum::<f64>())
        .collect();

    // Fit on training set.
    let train_wqs: Vec<f64> = train_idx.iter().map(|&i| wqs_index[i]).collect();
    let train_y: Vec<f64> = train_idx.iter().map(|&i| y[i]).collect();
    let mut train_preds: Vec<&[f64]> = vec![&train_wqs[..]];
    let train_covs_owned: Vec<Vec<f64>> = covariates
        .iter()
        .map(|c| train_idx.iter().map(|&i| c[i]).collect())
        .collect();
    for c in &train_covs_owned {
        train_preds.push(c.as_slice());
    }
    let fit_train = regression::logistic(&train_preds, &train_y, true)?;

    // Evaluate on test set.
    let test_wqs: Vec<f64> = test_idx.iter().map(|&i| wqs_index[i]).collect();
    let test_y: Vec<f64> = test_idx.iter().map(|&i| y[i]).collect();
    let mut test_preds: Vec<&[f64]> = vec![&test_wqs[..]];
    let test_covs_owned: Vec<Vec<f64>> = covariates
        .iter()
        .map(|c| test_idx.iter().map(|&i| c[i]).collect())
        .collect();
    for c in &test_covs_owned {
        test_preds.push(c.as_slice());
    }
    let fit_test = regression::logistic(&test_preds, &test_y, true).ok();

    let z_975 = 1.959963984540054;
    let beta1 = fit_train.coefficients[1]; // index 0=intercept, 1=wqs
    let se1 = fit_train.std_errors[1];
    let p1 = fit_train.p_values[1];
    let or = beta1.exp();
    let or_lo = (beta1 - z_975 * se1).exp();
    let or_hi = (beta1 + z_975 * se1).exp();

    let test_p = fit_test
        .map(|f| f.p_values.get(1).copied().unwrap_or(f64::NAN))
        .unwrap_or(f64::NAN);

    Ok(WqsResult {
        feature_names,
        weights: weights.clone(),
        bootstrap_mean_weights: boot_mean,
        bootstrap_se_weights: boot_se,
        beta_wqs: beta1,
        beta_wqs_se: se1,
        beta_wqs_p: p1,
        wqs_or: or,
        wqs_or_lower: or_lo,
        wqs_or_upper: or_hi,
        test_p_value: test_p,
        n_train,
        n_test: test_idx.len(),
    })
}

// ── Weight optimisation via projected gradient descent ─────────────────────

/// Optimise WQS weights on the training set via projected gradient descent
/// on the probability simplex.
///
/// Objective: maximise the log-likelihood of the logistic model
/// `logit(P(y=1)) = β₀ + β₁·(Σ wⱼ·qⱼ)` subject to `Σwⱼ=1`, `wⱼ≥0`.
///
/// We use a two-loop approach: for each set of candidate weights, compute the
/// WQS index, fit a logistic regression (β₀, β₁ only — no covariates in the
/// optimisation loop for speed), and use the gradient of the log-likelihood
/// w.r.t. weights to update. Projection onto the simplex follows Wang &
/// Carreira-Perpiñán (2013).
fn optimise_weights(
    q_exposures: &[Vec<f64>],
    y: &[f64],
    _covariates: &[&[f64]],
    train_idx: &[usize],
    opts: &WqsOptions,
) -> Result<Vec<f64>> {
    let k = q_exposures.len();
    let n_train = train_idx.len();

    // Extract training data.
    let q_train: Vec<Vec<f64>> = (0..k)
        .map(|j| train_idx.iter().map(|&i| q_exposures[j][i]).collect())
        .collect();
    let y_train: Vec<f64> = train_idx.iter().map(|&i| y[i]).collect();

    // Initialise uniform weights.
    let mut weights = vec![1.0 / k as f64; k];

    for _iter in 0..opts.max_iter {
        // Compute current WQS index.
        let wqs: Vec<f64> = (0..n_train)
            .map(|i| (0..k).map(|j| weights[j] * q_train[j][i]).sum::<f64>())
            .collect();

        // Fit logistic: y ~ wqs (intercept + slope).
        let fit = match regression::logistic(&[&wqs[..]], &y_train, true) {
            Ok(f) => f,
            Err(_) => break,
        };
        let beta1 = fit.coefficients[1];

        // Compute gradient of log-likelihood w.r.t. weights.
        // ∂LL/∂wⱼ = β₁ · Σᵢ (yᵢ − μᵢ) · qⱼ(Xᵢ)
        let grad: Vec<f64> = (0..k)
            .map(|j| {
                let g: f64 = (0..n_train)
                    .map(|i| (y_train[i] - fit.fitted[i]) * q_train[j][i])
                    .sum();
                beta1 * g
            })
            .collect();

        // Gradient ascent step.
        let sign = if opts.positive { 1.0 } else { -1.0 };
        let mut new_weights: Vec<f64> = (0..k)
            .map(|j| weights[j] + sign * opts.learning_rate * grad[j])
            .collect();

        // Enforce sign constraint: for positive direction, negative weights → 0.
        if opts.positive {
            for w in &mut new_weights {
                if *w < 0.0 {
                    *w = 0.0;
                }
            }
        }

        // Project onto simplex (Σw = 1).
        new_weights = project_simplex(&new_weights);

        // Convergence check.
        let max_change = (0..k)
            .map(|j| (new_weights[j] - weights[j]).abs())
            .fold(0.0_f64, f64::max);
        weights = new_weights;
        if max_change < opts.tol {
            break;
        }
    }

    Ok(weights)
}

/// Bootstrap weight stability: resample the training set B times, re-optimise
/// weights each time, and return mean ± SE.
fn bootstrap_weights(
    q_exposures: &[Vec<f64>],
    y: &[f64],
    _covariates: &[&[f64]],
    train_idx: &[usize],
    opts: &WqsOptions,
) -> Result<(Vec<f64>, Vec<f64>)> {
    let k = q_exposures.len();
    let n_train = train_idx.len();
    let mut rng = ChaCha8Rng::seed_from_u64(opts.seed.wrapping_add(1));

    // Collect all bootstrap weight vectors.
    let mut boot_weights: Vec<Vec<f64>> = Vec::with_capacity(opts.n_bootstrap);

    for _ in 0..opts.n_bootstrap {
        // Resample training indices with replacement.
        let boot_idx: Vec<usize> = (0..n_train)
            .map(|_| {
                let r: f64 = rng.random();
                train_idx[(r * n_train as f64) as usize]
            })
            .collect();

        // Build resampled exposures.
        let q_boot: Vec<Vec<f64>> = (0..k)
            .map(|j| boot_idx.iter().map(|&i| q_exposures[j][i]).collect())
            .collect();
        let y_boot: Vec<f64> = boot_idx.iter().map(|&i| y[i]).collect();

        let weights = optimise_weights_raw(&q_boot, &y_boot, opts);
        boot_weights.push(weights);
    }

    // Compute mean and SE for each weight.
    let mean: Vec<f64> = (0..k)
        .map(|j| {
            let sum: f64 = boot_weights.iter().map(|w| w[j]).sum();
            sum / opts.n_bootstrap as f64
        })
        .collect();
    let se: Vec<f64> = (0..k)
        .map(|j| {
            let m = mean[j];
            let ss: f64 = boot_weights.iter().map(|w| (w[j] - m).powi(2)).sum();
            (ss / (opts.n_bootstrap as f64 - 1.0)).sqrt()
        })
        .collect();

    Ok((mean, se))
}

/// Weight optimisation on raw (already-extracted) training arrays.
fn optimise_weights_raw(q_train: &[Vec<f64>], y_train: &[f64], opts: &WqsOptions) -> Vec<f64> {
    let k = q_train.len();
    let n_train = y_train.len();
    let mut weights = vec![1.0 / k as f64; k];

    for _ in 0..opts.max_iter {
        let wqs: Vec<f64> = (0..n_train)
            .map(|i| (0..k).map(|j| weights[j] * q_train[j][i]).sum::<f64>())
            .collect();

        let fit = match regression::logistic(&[&wqs[..]], y_train, true) {
            Ok(f) => f,
            Err(_) => break,
        };
        let beta1 = fit.coefficients[1];

        let grad: Vec<f64> = (0..k)
            .map(|j| {
                let g: f64 = (0..n_train)
                    .map(|i| (y_train[i] - fit.fitted[i]) * q_train[j][i])
                    .sum();
                beta1 * g
            })
            .collect();

        let sign = if opts.positive { 1.0 } else { -1.0 };
        let mut new_weights: Vec<f64> = (0..k)
            .map(|j| weights[j] + sign * opts.learning_rate * grad[j])
            .collect();

        if opts.positive {
            for w in &mut new_weights {
                if *w < 0.0 {
                    *w = 0.0;
                }
            }
        }
        new_weights = project_simplex(&new_weights);

        let max_change = (0..k)
            .map(|j| (new_weights[j] - weights[j]).abs())
            .fold(0.0_f64, f64::max);
        weights = new_weights;
        if max_change < opts.tol {
            break;
        }
    }

    weights
}

// ── Simplex projection ─────────────────────────────────────────────────────

/// Project a point onto the probability simplex {w ≥ 0, Σw = 1}.
///
/// Algorithm from Wang & Carreira-Perpiñán (2013): sort descending, find the
/// threshold ρ, then `w_i = max(v_i − ρ, 0)`.
fn project_simplex(v: &[f64]) -> Vec<f64> {
    let mut sorted = v.to_vec();
    sorted.sort_by(|a, b| b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal));

    let mut rho = 0;
    let mut cumsum = 0.0;
    for (i, &val) in sorted.iter().enumerate() {
        cumsum += val;
        if val - (cumsum - 1.0) / (i + 1) as f64 > 0.0 {
            rho = i;
        }
    }
    let theta = (sorted[..=rho].iter().sum::<f64>() - 1.0) / (rho + 1) as f64;

    v.iter().map(|&x| (x - theta).max(0.0)).collect()
}

// ── Quantile transform ─────────────────────────────────────────────────────

/// Transform a continuous variable into n_bins quantile categories (0..n_bins−1).
fn quantile_transform(x: &[f64], n_bins: usize) -> Vec<f64> {
    let n = x.len();
    if n == 0 || n_bins == 0 {
        return vec![];
    }

    // Sort and compute break points.
    let mut sorted = x.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

    let breaks: Vec<f64> = (1..n_bins)
        .map(|b| {
            let idx = (n as f64 * b as f64 / n_bins as f64).floor() as usize;
            sorted[idx.min(n - 1)]
        })
        .collect();

    // Assign each observation to a bin.
    x.iter()
        .map(|&v| {
            let bin = breaks.iter().filter(|&&br| v >= br).count();
            bin as f64
        })
        .collect()
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn approx_eq(a: f64, b: f64, tol: f64) -> bool {
        (a - b).abs() <= tol * b.abs().max(1.0)
    }

    #[test]
    fn simplex_projection_sums_to_one() {
        let v = vec![0.3, -0.5, 0.8, 0.1, 0.6];
        let w = project_simplex(&v);
        let sum: f64 = w.iter().sum();
        assert!(approx_eq(sum, 1.0, 1e-10));
        assert!(w.iter().all(|&x| x >= 0.0));
    }

    #[test]
    fn simplex_uniform_input_stays_uniform() {
        let v = vec![0.2; 5];
        let w = project_simplex(&v);
        for &x in &w {
            assert!(approx_eq(x, 0.2, 1e-10));
        }
    }

    #[test]
    fn quantile_transform_assigns_bins() {
        let x: Vec<f64> = (0..100).map(|i| i as f64).collect(); // 0..99
        let q = quantile_transform(&x, 4);
        assert_eq!(q.len(), 100);
        // Min value → bin 0, max value → bin 3.
        assert_eq!(q[0], 0.0);
        assert_eq!(q[99], 3.0);
        // All bins in [0, 3].
        assert!(q.iter().all(|&v| (0.0..=3.0).contains(&v)));
    }

    #[test]
    fn wqs_runs_with_dominant_exposure() {
        // x1 is strongly associated with y, x2 is noise.
        // WQS should give x1 a higher weight.
        let x1: Vec<f64> = (0..100)
            .map(|i| if i < 50 { 0.0 } else { 5.0 + i as f64 * 0.1 })
            .collect();
        let x2: Vec<f64> = (0..100).map(|i| (i as f64 * 0.3).sin()).collect();
        let y: Vec<f64> = (0..100).map(|i| if i < 50 { 0.0 } else { 1.0 }).collect();

        let opts = WqsOptions {
            n_bootstrap: 50, // small for test speed
            max_iter: 200,
            ..Default::default()
        };
        let result = wqs(
            &[&x1[..], &x2[..]],
            &y,
            &[],
            vec!["x1".into(), "x2".into()],
            &opts,
        )
        .unwrap();

        assert_eq!(result.weights.len(), 2);
        assert!(result.weights.iter().all(|&w| w >= 0.0));
        let sum: f64 = result.weights.iter().sum();
        assert!(approx_eq(sum, 1.0, 1e-6));
        // x1 should dominate.
        assert!(
            result.weights[0] > result.weights[1],
            "x1 weight ({}) should exceed x2 weight ({})",
            result.weights[0],
            result.weights[1]
        );
    }

    #[test]
    fn wqs_rejects_non_binary() {
        let x = [1.0, 2.0, 3.0];
        let y = vec![0.0, 0.5, 1.0];
        let opts = WqsOptions::default();
        assert!(wqs(&[&x[..]], &y, &[], vec!["x".into()], &opts).is_err());
    }

    #[test]
    fn wqs_returns_test_p_value() {
        let x1: Vec<f64> = (0..80).map(|i| i as f64).collect();
        let x2: Vec<f64> = (0..80).map(|i| (i as f64).cos()).collect();
        let y: Vec<f64> = (0..80).map(|i| if i < 40 { 0.0 } else { 1.0 }).collect();

        let opts = WqsOptions {
            n_bootstrap: 20,
            ..Default::default()
        };
        let result = wqs(
            &[&x1[..], &x2[..]],
            &y,
            &[],
            vec!["x1".into(), "x2".into()],
            &opts,
        )
        .unwrap();
        // test_p_value may be NaN if test fit fails, but should not panic.
        assert!(result.n_train + result.n_test == 80);
    }
}
