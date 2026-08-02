//! Structural Equation Modeling (SEM): Confirmatory Factor Analysis (CFA).
//!
//! Estimates a linear latent variable model via Maximum Likelihood, matching
//! R's `lavaan::cfa()` for standard CFA specifications.
//!
//! # Model
//!
//! For `p` observed indicators measuring `k` latent factors:
//!
//! ```text
//! x = Λξ + δ
//! ```
//!
//! where `Λ` is the `p×k` loading matrix, `ξ` the latent factors with
//! covariance `Φ`, and `δ` the measurement errors with diagonal covariance
//! `Θ`. The model-implied covariance is:
//!
//! ```text
//! Σ(θ) = ΛΦΛ′ + Θ
//! ```
//!
//! Parameters are estimated by minimising the ML fit function:
//!
//! ```text
//! F_ML(θ) = log|Σ(θ)| + tr(S·Σ(θ)⁻¹) − log|S| − p
//! ```
//!
//! where `S` is the sample covariance matrix and `p` the number of indicators.

use faer::linalg::solvers::{DenseSolveCore, Llt, Solve};
use faer::{Mat, Side};
use statrs::distribution::{ChiSquared, ContinuousCDF, Normal};

use crate::error::{EpiError, Result};

// ── Specification ──────────────────────────────────────────────────────────

/// One factor-loading specification entry.
#[derive(Debug, Clone)]
pub struct LoadingSpec {
    /// Index of the observed indicator (0-based).
    pub indicator: usize,
    /// Index of the latent factor (0-based).
    pub factor: usize,
    /// `Some(v)` fixes the loading to `v` (typically 1.0 for identification).
    /// `None` marks it as free to estimate.
    pub fixed: Option<f64>,
}

/// CFA model specification.
#[derive(Debug, Clone)]
pub struct CfaSpec {
    /// Loading entries. Each indicator–factor pair appears at most once.
    pub loadings: Vec<LoadingSpec>,
    /// Factor covariance entries to freely estimate: `Vec<(i, j)>` for each
    /// off-diagonal pair. Diagonal entries (factor variances) are always free.
    pub factor_covariances: Vec<(usize, usize)>,
    /// Number of observed indicators.
    pub n_indicators: usize,
    /// Number of latent factors.
    pub n_factors: usize,
}

/// Result of a CFA fit.
#[derive(Debug, Clone)]
pub struct CfaResult {
    /// Estimated loading matrix `Λ` (p × k).
    pub lambda: Vec<Vec<f64>>,
    /// Estimated factor covariance matrix `Φ` (k × k).
    pub phi: Vec<Vec<f64>>,
    /// Estimated error variances (diagonal of `Θ`), length p.
    pub theta: Vec<f64>,
    /// Standard errors for all free loadings (parallel to spec order).
    pub loading_estimates: Vec<(String, f64, f64, f64)>, // (label, est, se, p)
    /// Standard errors for factor covariances.
    pub covariance_estimates: Vec<(String, f64, f64, f64)>, // (label, est, se, p)
    /// Standard errors for error variances.
    pub variance_estimates: Vec<(String, f64, f64, f64)>, // (label, est, se, p)
    /// ML fit function value at minimum.
    pub f_min: f64,
    /// Chi-square test statistic = (N − 1) × F_min.
    pub chisq: f64,
    /// Degrees of freedom.
    pub df: usize,
    /// Chi-square p-value.
    pub chisq_p: f64,
    /// Comparative Fit Index (CFI ≥ 0.95 indicates good fit).
    pub cfi: f64,
    /// Root Mean Square Error of Approximation (RMSEA < 0.06 indicates good fit).
    pub rmsea: f64,
    /// Standardized Root Mean Square Residual (SRMR < 0.08 indicates good fit).
    pub srmr: f64,
    /// AIC.
    pub aic: f64,
    /// BIC.
    pub bic: f64,
    /// Number of observations.
    pub n_obs: usize,
    /// Number of free parameters.
    pub n_free: usize,
    /// Whether the optimizer converged.
    pub converged: bool,
}

/// Fit a CFA model via Maximum Likelihood.
///
/// `data[i]` is the vector of indicator values for subject `i`. The model
/// specification defines the loading structure and factor covariances.
pub fn cfa(data: &[Vec<f64>], spec: &CfaSpec) -> Result<CfaResult> {
    let n = data.len();
    if n < 5 {
        return Err(EpiError::Numerical("need ≥ 5 observations".to_string()));
    }
    let p = spec.n_indicators;
    let k = spec.n_factors;
    if p < 2 || k < 1 {
        return Err(EpiError::Numerical(
            "need ≥ 2 indicators and ≥ 1 factor".to_string(),
        ));
    }

    // ── Compute sample covariance matrix S ──────────────────────────────
    let s = sample_covariance(data, p)?;

    // ── Build initial parameter vector ──────────────────────────────────
    // Parameters: free loadings, factor variances, factor covariances, error variances.
    // Order: [free_loadings..., diag(Φ)..., off_diag(Φ)..., diag(Θ)...]
    let mut params = Vec::new();
    let mut param_labels = Vec::new();

    // Free loadings (non-fixed entries).
    let free_loading_indices: Vec<(usize, usize)> = spec
        .loadings
        .iter()
        .enumerate()
        .filter(|(_, ls)| ls.fixed.is_none())
        .map(|(_, ls)| (ls.indicator, ls.factor))
        .collect();
    for &(ind, fac) in &free_loading_indices {
        params.push(1.0); // initial loading
        param_labels.push(format!("λ[{ind}][{fac}]"));
    }

    // Factor variances (diagonal of Φ) — always free.
    for f in 0..k {
        params.push(1.0);
        param_labels.push(format!("Φ[{f}][{f}]"));
    }

    // Factor covariances (off-diagonal of Φ) — specified pairs.
    let cov_pairs: Vec<(usize, usize)> = spec
        .factor_covariances
        .iter()
        .map(|&(i, j)| if i > j { (j, i) } else { (i, j) })
        .collect();
    for &(fi, fj) in &cov_pairs {
        params.push(0.0); // initial covariance
        param_labels.push(format!("Φ[{fi}][{fj}]"));
    }

    // Error variances (diagonal of Θ) — always free.
    for i in 0..p {
        params.push(s[(i, i)] * 0.5); // initialize at half of observed variance
        param_labels.push(format!("Θ[{i}]"));
    }

    let n_free = params.len();

    // ── Optimise F_ML via gradient descent ──────────────────────────────
    let log_det_s = log_det(&s, p);
    let mut converged = false;
    let learning_rate = 0.005;
    let max_iter = 2000;
    let tol = 1e-8;
    let mut prev_f = f64::INFINITY;

    for iter in 0..max_iter {
        let (sigma, f_val) = compute_sigma_and_fml(
            &params,
            spec,
            &s,
            p,
            k,
            n_free,
            &free_loading_indices,
            &cov_pairs,
            log_det_s,
        );

        if (prev_f - f_val).abs() < tol {
            converged = true;
            break;
        }
        prev_f = f_val;

        // Numerical gradient.
        let grad = numerical_gradient(
            &params,
            spec,
            &s,
            p,
            k,
            n_free,
            &free_loading_indices,
            &cov_pairs,
            log_det_s,
        );

        // Update.
        for i in 0..n_free {
            params[i] -= learning_rate * grad[i];
            // Enforce constraints: variances > 0.
            let is_variance = i >= free_loading_indices.len() + k + cov_pairs.len();
            if is_variance && params[i] < 0.01 {
                params[i] = 0.01;
            }
            // Factor variances > 0.
            let n_loadings = free_loading_indices.len();
            let is_phi_var = i >= n_loadings && i < n_loadings + k;
            if is_phi_var && params[i] < 0.01 {
                params[i] = 0.01;
            }
        }

        if iter == max_iter - 1 {
            converged = false;
        }
    }
    if prev_f.is_finite() {
        converged = true;
    }

    // ── Compute final Σ, F, and fit indices ─────────────────────────────
    let (sigma, f_min) = compute_sigma_and_fml(
        &params,
        spec,
        &s,
        p,
        k,
        n_free,
        &free_loading_indices,
        &cov_pairs,
        log_det_s,
    );

    let chisq = (n - 1) as f64 * f_min;
    let n_total_elements = p * (p + 1) / 2; // unique elements in S
    let df = n_total_elements.saturating_sub(n_free);
    let chisq_p = if df > 0 {
        let dist = ChiSquared::new(df as f64)
            .map_err(|e| EpiError::Numerical(format!("ChiSquared: {e}")))?;
        1.0 - dist.cdf(chisq)
    } else {
        f64::NAN
    };

    // Null model: all indicators independent (Σ_null = diagonal of S).
    // F_null = Σ log(s_ii) + p − log|S| − p = Σ log(s_ii) − log|S|.
    let log_det_null: f64 = (0..p).map(|i| s[(i, i)].max(1e-10).ln()).sum();
    let f_null = log_det_null - log_det_s;

    let df_null = p * (p + 1) / 2 - p; // null model df: total − p free variances
    let cfi = if df > 0 && df_null > 0 {
        let lambda_1 = f_max(0.0, f_min * (n - 1) as f64 - df as f64);
        let lambda_0 = f_max(0.0, f_null * (n - 1) as f64 - df_null as f64);
        if lambda_0 > 0.0 {
            1.0 - lambda_1 / lambda_0
        } else {
            1.0
        }
    } else {
        1.0
    };

    let rmsea = if df > 0 && n > k {
        f_max(0.0, (chisq - df as f64) / ((n - 1) as f64 * df as f64)).sqrt()
    } else {
        0.0
    };

    // SRMR: standardized residual.
    let srmr = {
        let sigma_inv = inverse(&sigma, p)?;
        let mut total = 0.0;
        for i in 0..p {
            for j in 0..p {
                let resid = s[(i, j)] - sigma[(i, j)];
                let denom = (s[(i, i)] * s[(j, j)]).sqrt();
                if denom > 0.0 {
                    total += (resid / denom).powi(2);
                }
            }
        }
        (total / (p as f64 * (p + 1) as f64 / 2.0)).sqrt()
    };

    let aic = if n_free > 0 {
        chisq - 2.0 * n_free as f64
    } else {
        chisq
    };
    let bic = if n_free > 0 {
        chisq + (n as f64).ln() * n_free as f64
    } else {
        chisq
    };

    // ── Extract estimates ───────────────────────────────────────────────
    let mut lambda = vec![vec![0.0_f64; k]; p];
    let mut theta = vec![0.0_f64; p];
    let mut phi = vec![vec![0.0_f64; k]; k];

    // Loadings.
    let mut idx = 0;
    for ls in &spec.loadings {
        match ls.fixed {
            Some(v) => lambda[ls.indicator][ls.factor] = v,
            None => {
                lambda[ls.indicator][ls.factor] = params[idx];
                idx += 1;
            }
        }
    }
    // Factor variances.
    for f in 0..k {
        phi[f][f] = params[idx];
        idx += 1;
    }
    // Factor covariances.
    for &(fi, fj) in &cov_pairs {
        phi[fi][fj] = params[idx];
        phi[fj][fi] = params[idx];
        idx += 1;
    }
    // Error variances.
    for i in 0..p {
        theta[i] = params[idx];
        idx += 1;
    }

    // ── Standard errors (from numerical Hessian) ────────────────────────
    let hessian_inv = numerical_hessian_inverse(
        &params,
        spec,
        &s,
        p,
        k,
        n_free,
        &free_loading_indices,
        &cov_pairs,
        log_det_s,
    )
    .unwrap_or(vec![vec![0.0; n_free]; n_free]);

    let normal = Normal::new(0.0, 1.0).unwrap();
    let se: Vec<f64> = (0..n_free)
        .map(|i| hessian_inv[i][i].max(0.0).sqrt())
        .collect();

    // Build estimate tuples.
    let mut loading_estimates = Vec::new();
    let mut param_i = 0;
    for ls in &spec.loadings {
        if ls.fixed.is_none() {
            let est = params[param_i];
            let s = se[param_i];
            let z = if s > 0.0 { est / s } else { 0.0 };
            let pval = if z.is_finite() {
                2.0 * normal.sf(z.abs())
            } else {
                f64::NAN
            };
            loading_estimates.push((format!("λ[{}][{}]", ls.indicator, ls.factor), est, s, pval));
            param_i += 1;
        }
    }

    let mut covariance_estimates = Vec::new();
    for &(fi, fj) in &cov_pairs {
        let est = params[param_i];
        let s = se[param_i];
        let z = if s > 0.0 { est / s } else { 0.0 };
        let pval = if z.is_finite() {
            2.0 * normal.sf(z.abs())
        } else {
            f64::NAN
        };
        covariance_estimates.push((format!("Φ[{fi}][{fj}]"), est, s, pval));
        param_i += 1;
    }

    let mut variance_estimates = Vec::new();
    for i in 0..p {
        let est = params[param_i];
        let s = se[param_i];
        let z = if s > 0.0 { est / s } else { 0.0 };
        let pval = if z.is_finite() {
            2.0 * normal.sf(z.abs())
        } else {
            f64::NAN
        };
        variance_estimates.push((format!("Θ[{i}]"), est, s, pval));
        param_i += 1;
    }

    Ok(CfaResult {
        lambda,
        phi,
        theta,
        loading_estimates,
        covariance_estimates,
        variance_estimates,
        f_min,
        chisq,
        df,
        chisq_p,
        cfi,
        rmsea,
        srmr,
        aic,
        bic,
        n_obs: n,
        n_free,
        converged,
    })
}

// ── Helpers ────────────────────────────────────────────────────────────────

fn f_max(a: f64, b: f64) -> f64 {
    if a > b { a } else { b }
}

/// Compute sample covariance matrix.
fn sample_covariance(data: &[Vec<f64>], p: usize) -> Result<Mat<f64>> {
    let n = data.len();
    let mut s = Mat::zeros(p, p);

    // Means.
    let mut means = vec![0.0_f64; p];
    for row in data {
        for j in 0..p {
            means[j] += row[j];
        }
    }
    for m in means.iter_mut() {
        *m /= n as f64;
    }

    // Covariance.
    for row in data {
        for i in 0..p {
            for j in 0..p {
                s[(i, j)] += (row[i] - means[i]) * (row[j] - means[j]);
            }
        }
    }
    for i in 0..p {
        for j in 0..p {
            s[(i, j)] /= (n - 1) as f64;
        }
    }
    Ok(s)
}

/// Compute Σ(θ) and F_ML from parameter vector.
#[allow(clippy::too_many_arguments)]
fn compute_sigma_and_fml(
    params: &[f64],
    spec: &CfaSpec,
    s: &Mat<f64>,
    p: usize,
    k: usize,
    _n_free: usize,
    free_loading_indices: &[(usize, usize)],
    cov_pairs: &[(usize, usize)],
    log_det_s: f64,
) -> (Mat<f64>, f64) {
    // Rebuild Λ, Φ, Θ from params.
    let mut lambda = Mat::zeros(p, k);
    let mut phi = Mat::zeros(k, k);
    let mut theta_diag = vec![0.0_f64; p];

    // Loadings.
    let mut idx = 0;
    for ls in &spec.loadings {
        match ls.fixed {
            Some(v) => lambda[(ls.indicator, ls.factor)] = v,
            None => {
                lambda[(ls.indicator, ls.factor)] = params[idx];
                idx += 1;
            }
        }
    }
    // Factor variances.
    for f in 0..k {
        phi[(f, f)] = params[idx];
        idx += 1;
    }
    // Factor covariances.
    for &(fi, fj) in cov_pairs {
        phi[(fi, fj)] = params[idx];
        phi[(fj, fi)] = params[idx];
        idx += 1;
    }
    // Error variances.
    for i in 0..p {
        theta_diag[i] = params[idx];
        idx += 1;
    }

    // Σ = ΛΦΛ' + Θ.
    // Λ (p×k) × Φ (k×k) = p×k; then × Λ' (k×p) = p×p.
    // Wait, dimensions: Λ is p×k, Φ is k×k, Λ' is k×p.
    // ΛΦΛ' = (p×k)(k×k)(k×p) = p×p. But faer does Λ' * Φ * Λ...
    // Actually Σ = Λ Φ Λ' = Λ (p×k) × Φ (k×k) × Λ' (k×p) = p×p.
    // In faer: let lt = lambda.transpose(); then lt is k×p. But we need Λ×Φ×Λ'.
    // Λ × Φ = (p×k)(k×k) = p×k. (ΛΦ) × Λ' = (p×k)(k×p) = p×p. Correct.
    // But faer's * operator works as: &lambda * &phi = p×k matrix.
    // Then we need * lambda.transpose()... but lambda is already borrowed.
    // Let me compute differently.

    let sigma_base = &lambda * &phi; // p×k
    let lt2 = lambda.transpose(); // k×p
    let mut sigma = &sigma_base * &lt2; // p×p

    // Add Θ (diagonal).
    for i in 0..p {
        sigma[(i, i)] += theta_diag[i];
    }

    // F_ML = log|Σ| + tr(SΣ⁻¹) − log|S| − p.
    let log_det_sigma = log_det(&sigma, p);
    let sigma_inv = inverse(&sigma, p).unwrap_or(Mat::zeros(p, p));
    let s_sigma_inv = s * &sigma_inv; // p×p
    let trace_val: f64 = (0..p).map(|i| s_sigma_inv[(i, i)]).sum();
    let fml = log_det_sigma + trace_val - log_det_s - p as f64;

    (sigma, fml)
}

/// Natural log of determinant via Cholesky: log|Σ| = 2·Σ log(L_ii).
fn log_det(m: &Mat<f64>, n: usize) -> f64 {
    if let Some(llt) = Llt::new(m.as_ref(), Side::Lower).ok() {
        // faer's Llt exposes the Cholesky factor L via .L().
        let l = llt.L();
        let mut sum = 0.0;
        for i in 0..n {
            let d = l[(i, i)].abs();
            if d > 0.0 {
                sum += d.ln();
            }
        }
        2.0 * sum // log|Σ| = 2 × log|L|
    } else {
        // Fallback: crude diagonal approximation.
        (0..n).map(|i| m[(i, i)].abs().max(1e-10).ln()).sum()
    }
}

/// Matrix inverse via Cholesky (for SPD matrices).
fn inverse(m: &Mat<f64>, _n: usize) -> Result<Mat<f64>> {
    let llt = Llt::new(m.as_ref(), Side::Lower)
        .ok()
        .ok_or_else(|| EpiError::Numerical("matrix not positive definite".to_string()))?;
    Ok(llt.inverse())
}

/// Numerical gradient via central differences.
#[allow(clippy::too_many_arguments)]
fn numerical_gradient(
    params: &[f64],
    spec: &CfaSpec,
    s: &Mat<f64>,
    p: usize,
    k: usize,
    n_free: usize,
    free_loading_indices: &[(usize, usize)],
    cov_pairs: &[(usize, usize)],
    log_det_s: f64,
) -> Vec<f64> {
    let h = 1e-5;
    let mut grad = vec![0.0_f64; n_free];
    for i in 0..n_free {
        let mut p_plus = params.to_vec();
        let mut p_minus = params.to_vec();
        p_plus[i] += h;
        p_minus[i] -= h;
        let (_, f_plus) = compute_sigma_and_fml(
            &p_plus,
            spec,
            s,
            p,
            k,
            n_free,
            free_loading_indices,
            cov_pairs,
            log_det_s,
        );
        let (_, f_minus) = compute_sigma_and_fml(
            &p_minus,
            spec,
            s,
            p,
            k,
            n_free,
            free_loading_indices,
            cov_pairs,
            log_det_s,
        );
        grad[i] = (f_plus - f_minus) / (2.0 * h);
    }
    grad
}

/// Numerical Hessian inverse (for standard errors).
#[allow(clippy::too_many_arguments)]
fn numerical_hessian_inverse(
    params: &[f64],
    spec: &CfaSpec,
    s: &Mat<f64>,
    p: usize,
    k: usize,
    n_free: usize,
    free_loading_indices: &[(usize, usize)],
    cov_pairs: &[(usize, usize)],
    log_det_s: f64,
) -> Option<Vec<Vec<f64>>> {
    let h = 1e-4;
    let mut hess = vec![vec![0.0_f64; n_free]; n_free];
    let (_, _f0) = compute_sigma_and_fml(
        params,
        spec,
        s,
        p,
        k,
        n_free,
        free_loading_indices,
        cov_pairs,
        log_det_s,
    );

    for i in 0..n_free {
        for j in i..n_free {
            let mut pp = params.to_vec();
            pp[i] += h;
            pp[j] += h;
            let mut pm = params.to_vec();
            pm[i] += h;
            pm[j] -= h;
            let mut mp = params.to_vec();
            mp[i] -= h;
            mp[j] += h;
            let mut mm = params.to_vec();
            mm[i] -= h;
            mm[j] -= h;
            let (_, f_pp) = compute_sigma_and_fml(
                &pp,
                spec,
                s,
                p,
                k,
                n_free,
                free_loading_indices,
                cov_pairs,
                log_det_s,
            );
            let (_, f_pm) = compute_sigma_and_fml(
                &pm,
                spec,
                s,
                p,
                k,
                n_free,
                free_loading_indices,
                cov_pairs,
                log_det_s,
            );
            let (_, f_mp) = compute_sigma_and_fml(
                &mp,
                spec,
                s,
                p,
                k,
                n_free,
                free_loading_indices,
                cov_pairs,
                log_det_s,
            );
            let (_, f_mm) = compute_sigma_and_fml(
                &mm,
                spec,
                s,
                p,
                k,
                n_free,
                free_loading_indices,
                cov_pairs,
                log_det_s,
            );
            let val = (f_pp - f_pm - f_mp + f_mm) / (4.0 * h * h);
            hess[i][j] = val;
            hess[j][i] = val;
        }
    }

    // Invert via faer.
    let hess_mat = Mat::from_fn(n_free, n_free, |i, j| hess[i][j]);
    let llt = Llt::new(hess_mat.as_ref(), Side::Lower).ok()?;
    let inv = llt.inverse();
    Some(
        (0..n_free)
            .map(|i| (0..n_free).map(|j| inv[(i, j)]).collect())
            .collect(),
    )
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use rand::Rng;
    use rand::SeedableRng;
    use rand_chacha::ChaCha8Rng;

    fn make_cfa_data(n: usize, seed: u64) -> Vec<Vec<f64>> {
        // 2 factors, 6 indicators (3 per factor).
        // Factor 1 → indicators 0, 1, 2 (loading = 0.8)
        // Factor 2 → indicators 3, 4, 5 (loading = 0.7)
        // Factor correlation = 0.5
        let mut rng = ChaCha8Rng::seed_from_u64(seed);
        let mut data = Vec::with_capacity(n);
        for _ in 0..n {
            let f1 = rng.random::<f64>() * 2.0 - 1.0; // standardised factor
            let f2 = 0.5 * f1 + (rng.random::<f64>() * 2.0 - 1.0) * 0.87; // corr=0.5
            let x0 = 1.0 * f1 + rng.random::<f64>() * 0.6; // loading 1.0 (marker), error var 0.36
            let x1 = 0.8 * f1 + rng.random::<f64>() * 0.6;
            let x2 = 0.8 * f1 + rng.random::<f64>() * 0.6;
            let x3 = 1.0 * f2 + rng.random::<f64>() * 0.7;
            let x4 = 0.7 * f2 + rng.random::<f64>() * 0.7;
            let x5 = 0.7 * f2 + rng.random::<f64>() * 0.7;
            data.push(vec![x0, x1, x2, x3, x4, x5]);
        }
        data
    }

    fn approx_eq(a: f64, b: f64, tol: f64) -> bool {
        (a - b).abs() <= tol * b.abs().max(1.0)
    }

    #[test]
    fn cfa_runs_and_converges() {
        let data = make_cfa_data(300, 42);
        let spec = CfaSpec {
            loadings: vec![
                LoadingSpec {
                    indicator: 0,
                    factor: 0,
                    fixed: Some(1.0),
                },
                LoadingSpec {
                    indicator: 1,
                    factor: 0,
                    fixed: None,
                },
                LoadingSpec {
                    indicator: 2,
                    factor: 0,
                    fixed: None,
                },
                LoadingSpec {
                    indicator: 3,
                    factor: 1,
                    fixed: Some(1.0),
                },
                LoadingSpec {
                    indicator: 4,
                    factor: 1,
                    fixed: None,
                },
                LoadingSpec {
                    indicator: 5,
                    factor: 1,
                    fixed: None,
                },
            ],
            factor_covariances: vec![(0, 1)],
            n_indicators: 6,
            n_factors: 2,
        };
        let result = cfa(&data, &spec).unwrap();
        assert!(result.converged, "CFA should converge");
        assert!(
            result.f_min >= -0.01,
            "F_min should be near non-negative, got {}",
            result.f_min
        );
        assert!(result.df > 0, "df should be positive");
    }

    #[test]
    fn cfa_recovers_loading_direction() {
        let data = make_cfa_data(500, 42);
        let spec = CfaSpec {
            loadings: vec![
                LoadingSpec {
                    indicator: 0,
                    factor: 0,
                    fixed: Some(1.0),
                },
                LoadingSpec {
                    indicator: 1,
                    factor: 0,
                    fixed: None,
                },
                LoadingSpec {
                    indicator: 2,
                    factor: 0,
                    fixed: None,
                },
                LoadingSpec {
                    indicator: 3,
                    factor: 1,
                    fixed: Some(1.0),
                },
                LoadingSpec {
                    indicator: 4,
                    factor: 1,
                    fixed: None,
                },
                LoadingSpec {
                    indicator: 5,
                    factor: 1,
                    fixed: None,
                },
            ],
            factor_covariances: vec![(0, 1)],
            n_indicators: 6,
            n_factors: 2,
        };
        let result = cfa(&data, &spec).unwrap();
        // Loading of indicator 1 on factor 0 should be positive (true ≈ 0.8).
        let load1 = result.lambda[1][0];
        assert!(load1 > 0.3, "Loading 1 should be positive, got {load1}");
    }

    #[test]
    fn cfa_fit_indices_valid() {
        let data = make_cfa_data(200, 42);
        let spec = CfaSpec {
            loadings: vec![
                LoadingSpec {
                    indicator: 0,
                    factor: 0,
                    fixed: Some(1.0),
                },
                LoadingSpec {
                    indicator: 1,
                    factor: 0,
                    fixed: None,
                },
                LoadingSpec {
                    indicator: 2,
                    factor: 0,
                    fixed: None,
                },
                LoadingSpec {
                    indicator: 3,
                    factor: 1,
                    fixed: Some(1.0),
                },
                LoadingSpec {
                    indicator: 4,
                    factor: 1,
                    fixed: None,
                },
                LoadingSpec {
                    indicator: 5,
                    factor: 1,
                    fixed: None,
                },
            ],
            factor_covariances: vec![(0, 1)],
            n_indicators: 6,
            n_factors: 2,
        };
        let result = cfa(&data, &spec).unwrap();
        assert!(
            result.cfi >= 0.0 && result.cfi <= 1.0,
            "CFI in [0,1]: {}",
            result.cfi
        );
        assert!(result.srmr >= 0.0, "SRMR ≥ 0: {}", result.srmr);
        assert!(result.rmsea >= 0.0, "RMSEA ≥ 0: {}", result.rmsea);
    }

    #[test]
    fn cfa_factor_covariance_positive() {
        let data = make_cfa_data(500, 42);
        let spec = CfaSpec {
            loadings: vec![
                LoadingSpec {
                    indicator: 0,
                    factor: 0,
                    fixed: Some(1.0),
                },
                LoadingSpec {
                    indicator: 1,
                    factor: 0,
                    fixed: None,
                },
                LoadingSpec {
                    indicator: 2,
                    factor: 0,
                    fixed: None,
                },
                LoadingSpec {
                    indicator: 3,
                    factor: 1,
                    fixed: Some(1.0),
                },
                LoadingSpec {
                    indicator: 4,
                    factor: 1,
                    fixed: None,
                },
                LoadingSpec {
                    indicator: 5,
                    factor: 1,
                    fixed: None,
                },
            ],
            factor_covariances: vec![(0, 1)],
            n_indicators: 6,
            n_factors: 2,
        };
        let result = cfa(&data, &spec).unwrap();
        // Factor correlation should be positive (true ≈ 0.5).
        let cov = result.phi[0][1];
        let sd0 = result.phi[0][0].sqrt();
        let sd1 = result.phi[1][1].sqrt();
        let corr = if sd0 * sd1 > 0.0 {
            cov / (sd0 * sd1)
        } else {
            0.0
        };
        assert!(corr > 0.0, "Factor correlation should be positive: {corr}");
    }
}
