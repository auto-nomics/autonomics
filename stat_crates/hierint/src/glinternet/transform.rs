//! Linear-predictor and gradient computation — port of the C kernels in `fista.c`.
//!
//! The glinternet C code never constructs an explicit design matrix for
//! categorical variables. Instead it stores integer codes and dispatches by
//! group type. This module faithfully reproduces that approach.

use super::{ActiveSet, GlinternetData};

/// Compute `linear += X·beta` where X and beta are structured by group type.
///
/// Port of `x_times_beta()` in `fista.c`. `beta` is a flat vector of coefficients
/// laid out as [cat_groups... | cont_groups... | catcat... | contcont... | catcont...].
/// `linear` is added to in-place.
pub fn x_times_beta(data: &GlinternetData, active: &ActiveSet, beta: &[f64], linear: &mut [f64]) {
    let n = data.n;
    let eps = 1e-12;
    let mut offset = 0usize;

    // ── Categorical main effects ────────────────────────────────────────
    if let Some(ref cat) = active.cat {
        let factor = (n as f64).sqrt();
        for &[ci] in cat {
            let n_levels = data.levels[ci - 1];
            // Check if all zero
            let all_zero = (0..n_levels).all(|i| beta[offset + i].abs() <= eps);
            if !all_zero {
                let xptr = &data.xcat[(ci - 1) * n..ci * n];
                for i in 0..n {
                    linear[i] += beta[offset + xptr[i]] / factor;
                }
            }
            offset += n_levels;
        }
    }

    // ── Continuous main effects ─────────────────────────────────────────
    if let Some(ref cont) = active.cont {
        for &[ci] in cont {
            if beta[offset].abs() > eps {
                let zptr = &data.z[(ci - 1) * n..ci * n];
                for i in 0..n {
                    linear[i] += zptr[i] * beta[offset];
                }
            }
            offset += 1;
        }
    }

    // ── Categorical × Categorical interactions ─────────────────────────
    if let Some(ref catcat) = active.catcat {
        let factor = (n as f64).sqrt();
        for &[ci, cj] in catcat {
            let l1 = data.levels[ci - 1];
            let l2 = data.levels[cj - 1];
            let len = l1 * l2;
            let all_zero = (0..len).all(|i| beta[offset + i].abs() <= eps);
            if !all_zero {
                let xptr = &data.xcat[(ci - 1) * n..ci * n];
                let yptr = &data.xcat[(cj - 1) * n..cj * n];
                for i in 0..n {
                    linear[i] += beta[offset + xptr[i] + l1 * yptr[i]] / factor;
                }
            }
            offset += len;
        }
    }

    // ── Continuous × Continuous interactions ───────────────────────────
    if let Some(ref contcont) = active.contcont {
        let factor = 3.0f64.sqrt();
        for &[ci, cj] in contcont {
            let all_zero = (0..3).all(|i| beta[offset + i].abs() <= eps);
            if !all_zero {
                let wptr = &data.z[(ci - 1) * n..ci * n];
                let zptr = &data.z[(cj - 1) * n..cj * n];
                for i in 0..n {
                    linear[i] += (wptr[i] * beta[offset] + zptr[i] * beta[offset + 1]) / factor;
                }
                // Product term (centered)
                let mut mean = 0.0;
                let mut norm_sq = 0.0;
                for i in 0..n {
                    let prod = wptr[i] * zptr[i];
                    mean += prod;
                    norm_sq += prod * prod;
                }
                if norm_sq > 0.0 {
                    mean /= n as f64;
                    let norm = (3.0 * (norm_sq - n as f64 * mean * mean)).sqrt();
                    for i in 0..n {
                        let prod = wptr[i] * zptr[i];
                        linear[i] += (prod - mean) * beta[offset + 2] / norm;
                    }
                }
            }
            offset += 3;
        }
    }

    // ── Categorical × Continuous interactions ──────────────────────────
    if let Some(ref catcont) = active.catcont {
        let factor = (2.0 * n as f64).sqrt();
        let factor_z = 2.0f64.sqrt();
        for &[ci, cj] in catcont {
            let n_levels = data.levels[ci - 1];
            let len = 2 * n_levels;
            let all_zero = (0..len).all(|i| beta[offset + i].abs() <= eps);
            if !all_zero {
                let xptr = &data.xcat[(ci - 1) * n..ci * n];
                let zptr = &data.z[(cj - 1) * n..cj * n];
                for i in 0..n {
                    linear[i] += beta[offset + xptr[i]] / factor;
                    linear[i] += zptr[i] * beta[offset + n_levels + xptr[i]] / factor_z;
                }
            }
            offset += len;
        }
    }
}

/// Compute gradient of the loss w.r.t. beta.
///
/// Port of `compute_gradient()` in `fista.c`.
/// The gradient is `X^T(Y - Xβ)` for the *residual* `r = Y - Xβ`.
/// The result is divided by `-n` at the end (matching C: gradient = X^T·r / -n
/// with per-type Frobenius normalization).
pub fn compute_gradient(data: &GlinternetData, active: &ActiveSet, residual: &[f64]) -> Vec<f64> {
    let n = data.n;
    let beta_len = active.beta_len(&data.levels);
    let mut grad = vec![0.0f64; beta_len];
    let mut offset = 0usize;

    // ── Categorical ─────────────────────────────────────────────────────
    if let Some(ref cat) = active.cat {
        let factor = (n as f64).sqrt();
        let start = offset;
        for &[ci] in cat {
            let n_levels = data.levels[ci - 1];
            let xptr = &data.xcat[(ci - 1) * n..ci * n];
            for i in 0..n {
                grad[offset + xptr[i]] += residual[i];
            }
            offset += n_levels;
        }
        for i in start..offset {
            grad[i] /= factor;
        }
    }

    // ── Continuous ──────────────────────────────────────────────────────
    if let Some(ref cont) = active.cont {
        for &[ci] in cont {
            let zptr = &data.z[(ci - 1) * n..ci * n];
            for i in 0..n {
                grad[offset] += zptr[i] * residual[i];
            }
            offset += 1;
        }
    }

    // ── Categorical × Categorical ───────────────────────────────────────
    if let Some(ref catcat) = active.catcat {
        let factor = (n as f64).sqrt();
        let start = offset;
        for &[ci, cj] in catcat {
            let l1 = data.levels[ci - 1];
            let xptr = &data.xcat[(ci - 1) * n..ci * n];
            let yptr = &data.xcat[(cj - 1) * n..cj * n];
            for i in 0..n {
                grad[offset + xptr[i] + l1 * yptr[i]] += residual[i];
            }
            offset += l1 * data.levels[cj - 1];
        }
        for i in start..offset {
            grad[i] /= factor;
        }
    }

    // ── Continuous × Continuous ─────────────────────────────────────────
    if let Some(ref contcont) = active.contcont {
        let factor = 3.0f64.sqrt();
        for &[ci, cj] in contcont {
            let wptr = &data.z[(ci - 1) * n..ci * n];
            let zptr = &data.z[(cj - 1) * n..cj * n];
            for i in 0..n {
                grad[offset] += wptr[i] * residual[i];
                grad[offset + 1] += zptr[i] * residual[i];
            }
            grad[offset] /= factor;
            grad[offset + 1] /= factor;

            let mut mean = 0.0;
            let mut norm_sq = 0.0;
            for i in 0..n {
                let prod = wptr[i] * zptr[i];
                mean += prod;
                norm_sq += prod * prod;
            }
            if norm_sq > 0.0 {
                mean /= n as f64;
                let norm = (3.0 * (norm_sq - n as f64 * mean * mean)).sqrt();
                for i in 0..n {
                    let prod = wptr[i] * zptr[i];
                    grad[offset + 2] += (prod - mean) * residual[i];
                }
                grad[offset + 2] /= norm;
            }
            offset += 3;
        }
    }

    // ── Categorical × Continuous ────────────────────────────────────────
    if let Some(ref catcont) = active.catcont {
        let factor = (2.0 * n as f64).sqrt();
        let factor_z = 2.0f64.sqrt();
        for &[ci, cj] in catcont {
            let n_levels = data.levels[ci - 1];
            let xptr = &data.xcat[(ci - 1) * n..ci * n];
            let zptr = &data.z[(cj - 1) * n..cj * n];
            for i in 0..n {
                grad[offset + xptr[i]] += residual[i];
                grad[offset + n_levels + xptr[i]] += zptr[i] * residual[i];
            }
            for i in offset..offset + n_levels {
                grad[i] /= factor;
            }
            for i in offset + n_levels..offset + 2 * n_levels {
                grad[i] /= factor_z;
            }
            offset += 2 * n_levels;
        }
    }

    // Normalize by -n
    for g in grad.iter_mut() {
        *g /= -(n as f64);
    }

    grad
}

/// Compute negative log-likelihood from linear predictor.
///
/// Port of `compute_loglik()` in `fista.c`.
pub fn compute_loglik(
    y_or_res: &[f64],
    linear: &[f64],
    intercept: f64,
    n: usize,
    family: super::Family,
) -> f64 {
    let mut result = 0.0;
    if family.is_gaussian() {
        for i in 0..n {
            let d = y_or_res[i] - intercept - linear[i];
            result += d * d;
        }
        result /= 2.0 * n as f64;
    } else {
        for i in 0..n {
            let z = intercept + linear[i];
            result += -y_or_res[i] * z + softplus(z);
        }
        result /= n as f64;
    }
    result
}

/// Numerically stable log(1 + exp(x)).
#[inline]
fn softplus(x: f64) -> f64 {
    if x > 30.0 {
        x
    } else if x < -30.0 {
        (-x).exp()
    } else {
        (1.0 + x.exp()).ln()
    }
}

/// Group soft-thresholding (proximal step).
///
/// Port of `compute_update()` in `fista.c`.
/// For each group i: `βᵢ_new = (βᵢ - step·gradᵢ) * max(0, 1 - step·λ/‖βᵢ - step·gradᵢ‖)`
pub fn compute_update(
    beta: &[f64],
    gradient: &[f64],
    group_sizes: &[usize],
    step: f64,
    lambda: f64,
) -> Vec<f64> {
    let factor = step * lambda;
    let mut updated = vec![0.0f64; beta.len()];
    let mut offset = 0usize;

    for &size in group_sizes {
        let mut norm_sq = 0.0;
        for j in 0..size {
            updated[offset + j] = beta[offset + j] - step * gradient[offset + j];
            norm_sq += updated[offset + j] * updated[offset + j];
        }
        let norm = norm_sq.sqrt();
        let shrink = (1.0 - factor / norm).max(0.0);
        for j in 0..size {
            updated[offset + j] *= shrink;
        }
        offset += size;
    }

    updated
}

/// Compute objective value (loss + penalty).
///
/// Port of `compute_objective()` in `fista.c`.
pub fn compute_objective(
    y: &[f64],
    residual: &[f64],
    linear: &[f64],
    intercept: f64,
    beta: &[f64],
    group_sizes: &[usize],
    lambda: f64,
    n: usize,
    family: super::Family,
) -> f64 {
    // Loss
    let loglik = if family.is_gaussian() {
        let mut sse = 0.0;
        for i in 0..n {
            sse += residual[i] * residual[i];
        }
        sse / (2.0 * n as f64)
    } else {
        let mut loss = 0.0;
        for i in 0..n {
            let z = intercept + linear[i];
            loss += -y[i] * z + softplus(z);
        }
        loss / n as f64
    };

    // Penalty: sum of group norms
    let mut penalty = 0.0;
    let mut offset = 0usize;
    for &size in group_sizes {
        let mut norm_sq = 0.0;
        for j in 0..size {
            norm_sq += beta[offset + j] * beta[offset + j];
        }
        penalty += norm_sq.sqrt();
        offset += size;
    }

    loglik + penalty * lambda
}

/// Check convergence via KKT conditions.
///
/// Port of `check_convergence()` in `fista.c`.
pub fn check_convergence(
    beta: &[f64],
    gradient: &[f64],
    group_sizes: &[usize],
    lambda: f64,
    tol: f64,
) -> bool {
    let eps = 1e-12;
    let mut offset = 0usize;

    for &size in group_sizes {
        // Check if beta is all zero
        let all_zero = (0..size).all(|j| beta[offset + j].abs() <= eps);

        // Gradient norm for this group
        let mut norm_sq = 0.0;
        for j in 0..size {
            norm_sq += gradient[offset + j] * gradient[offset + j];
        }
        let norm = norm_sq.sqrt();

        if !all_zero {
            // Nonzero group: check KKT |norm - lambda| / lambda <= tol
            if (norm - lambda).abs() / lambda > tol {
                return false;
            }
        } else {
            // Zero group: gradient norm must be <= lambda
            if norm > lambda {
                return false;
            }
        }
        offset += size;
    }
    true
}

/// Update intercept. Gaussian: shift by mean residual. Logistic: Newton.
///
/// Port of `update_intercept()` in `fista.c`.
pub fn update_intercept(
    y: &[f64],
    linear: &[f64],
    intercept: &mut f64,
    residual: &mut [f64],
    n: usize,
    family: super::Family,
) {
    if family.is_gaussian() {
        let mut residual_mean = 0.0;
        let mu = *intercept;
        for i in 0..n {
            residual[i] = y[i] - mu - linear[i];
            residual_mean += residual[i];
        }
        residual_mean /= n as f64;
        *intercept += residual_mean;
        for i in 0..n {
            residual[i] -= residual_mean;
        }
    } else {
        // Logistic: Newton on the intercept
        let xmax = -f64::EPSILON.ln();
        let xmin = f64::MIN_POSITIVE.ln();
        let mut mu = *intercept;
        let mut temp = vec![0.0; n];
        let mut exponent = vec![0.0; n];

        let exp_mu = (-mu).exp();
        let mut f = 0.0;
        let mut sum_y = 0.0;
        for i in 0..n {
            exponent[i] = (-linear[i]).exp();
            temp[i] = exp_mu * exponent[i];
            sum_y += y[i];
            let s = mu + linear[i];
            let p = if s > xmax {
                1.0
            } else if s < xmin {
                0.0
            } else {
                1.0 / (1.0 + temp[i])
            };
            f += y[i] - p;
        }

        let mut iter = 0;
        while iter < 1000 && f.abs() > 1e-2 {
            let mut f_prime = 0.0;
            for i in 0..n {
                let s = mu + linear[i];
                if s > xmax || s < xmin {
                    // f_prime contribution is 0
                } else {
                    f_prime -= temp[i] / (1.0 + temp[i]).powi(2);
                }
            }
            mu -= f / f_prime;
            let exp_mu = (-mu).exp();
            f = sum_y;
            for i in 0..n {
                temp[i] = exp_mu * exponent[i];
                let s = mu + linear[i];
                let p = if s > xmax {
                    1.0
                } else if s < xmin {
                    0.0
                } else {
                    1.0 / (1.0 + temp[i])
                };
                f -= p;
            }
            iter += 1;
        }

        *intercept = mu;
        for i in 0..n {
            let s = mu + linear[i];
            let p = if s > xmax {
                1.0
            } else if s < xmin {
                0.0
            } else {
                1.0 / (1.0 + temp[i])
            };
            residual[i] = y[i] - p;
        }
    }
}
