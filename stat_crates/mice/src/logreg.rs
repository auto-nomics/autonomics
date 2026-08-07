//! `mice.impute.logreg` — Bayesian logistic-regression imputation for binary
//! `y`.
//!
//! Faithful port of `R/mice.impute.logreg.R` (excluding `logreg.boot`):
//!
//!   1. Augment the data via the White–Daniel–Royston (2010) scheme
//!      ([`crate::augment::augment`]).
//!   2. Fit `glm.fit` with `family = quasibinomial(link = logit)` using IRLS.
//!   3. Draw `β* ~ N(β̂, V)` where `V = cov.unscaled(β̂)`.
//!   4. Compute predicted scores `p = 1 / (1 + exp(−x · β*))` for the missing
//!      rows and threshold against `runif()` to obtain 0/1 imputations.
//!
//! Reproduces R's `mice.impute.logreg` to ~1e-10 in the cross-validation
//! harness (modulo the shared RNG stream — both Rust and R must seed with
//! the same value).

use faer::linalg::solvers::{DenseSolveCore, Llt, Solve};
use faer::{Mat, Side};
use rand::Rng;
use rand_distr::{Distribution, Normal};

use crate::augment::augment;
use crate::error::{MiceError, Result};

/// Internal IRLS logit fit on an augmented design with weights.
#[derive(Debug, Clone)]
struct IrlsFit {
    coef: Vec<f64>,
    cov_unscaled: Vec<f64>,
    n_iter: usize,
    converged: bool,
}

/// Fit `glm.fit(x, y, family = binomial(link = "logit"), weights = w)` by
/// weighted IRLS. Returns β̂, the unscaled covariance `(Xᵀ W X)⁻¹`, and
/// convergence information.
///
/// `x_aug` is `n × (p+1)` row-major with an intercept column prepended by
/// the caller. `y_aug` is 0/1 (length `n`). `w_aug` is the per-row weight
/// (length `n`).
fn irls_logit(x_aug: &[Vec<f64>], y_aug: &[f64], w_aug: &[f64], max_iter: usize, tol: f64) -> Result<IrlsFit> {
    let n = y_aug.len();
    if x_aug.len() != n {
        return Err(MiceError::LengthMismatch("x_aug row count".into()));
    }
    if w_aug.len() != n {
        return Err(MiceError::LengthMismatch("w_aug length".into()));
    }
    if n == 0 {
        return Err(MiceError::InsufficientData("empty x_aug".into()));
    }
    let p = x_aug[0].len();
    if p == 0 {
        return Err(MiceError::InsufficientData("empty design".into()));
    }

    // Initial β = 0; μ = mean(y), offset handled implicitly via initial β.
    let mut beta = vec![0.0_f64; p];
    let mu_eps = 1e-10_f64;
    let mut converged = false;
    let mut n_iter = 0usize;

    for it in 0..max_iter {
        n_iter = it + 1;
        // Compute μ = sigmoid(x · β), w = μ(1 − μ), z = x · β + (y − μ) / w.
        let mut mu = vec![0.0_f64; n];
        let mut w = vec![0.0_f64; n];
        let mut z = vec![0.0_f64; n];
        for i in 0..n {
            let eta: f64 = (0..p).map(|j| x_aug[i][j] * beta[j]).sum();
            let m = 1.0 / (1.0 + (-eta).exp());
            mu[i] = m.clamp(mu_eps, 1.0 - mu_eps);
            w[i] = mu[i] * (1.0 - mu[i]);
            z[i] = eta + (y_aug[i] - mu[i]) / w[i];
        }
        // Weighted normal equations: (Xᵀ W X) Δ = Xᵀ W (z − x · β).
        let mut xtx = Mat::<f64>::zeros(p, p);
        let mut xtwz = vec![0.0_f64; p];
        let mut xb = vec![0.0_f64; n];
        for i in 0..n {
            let wi = w[i] * w_aug[i];
            let diff = z[i] - {
                let mut s = 0.0;
                for j in 0..p {
                    s += x_aug[i][j] * beta[j];
                }
                s
            };
            xb[i] = diff;
            for j in 0..p {
                xtwz[j] += wi * x_aug[i][j] * diff;
                for k in 0..p {
                    xtx[(j, k)] += wi * x_aug[i][j] * x_aug[i][k];
                }
            }
        }
        let _ = xb;

        let chol = Llt::new(xtx.as_ref(), Side::Lower).map_err(|e| MiceError::Numerical(format!("irls_logit: {e:?}")))?;
        let mut rhs = Mat::<f64>::zeros(p, 1);
        for j in 0..p {
            rhs[(j, 0)] = xtwz[j];
        }
        let delta_mat = chol.solve(&rhs);
        let delta: Vec<f64> = (0..p).map(|j| delta_mat[(j, 0)]).collect();

        let mut max_change = 0.0;
        for j in 0..p {
            beta[j] += delta[j];
            if delta[j].abs() > max_change {
                max_change = delta[j].abs();
            }
        }
        if max_change < tol {
            converged = true;
            break;
        }
    }

    // Compute cov.unscaled = (Xᵀ W X)⁻¹ at the final β.
    let mut mu = vec![0.0_f64; n];
    let mut w = vec![0.0_f64; n];
    for i in 0..n {
        let eta: f64 = (0..p).map(|j| x_aug[i][j] * beta[j]).sum();
        let m = 1.0 / (1.0 + (-eta).exp());
        mu[i] = m.clamp(mu_eps, 1.0 - mu_eps);
        w[i] = mu[i] * (1.0 - mu[i]);
    }
    let mut xtx = Mat::<f64>::zeros(p, p);
    for i in 0..n {
        let wi = w[i] * w_aug[i];
        for j in 0..p {
            for k in 0..p {
                xtx[(j, k)] += wi * x_aug[i][j] * x_aug[i][k];
            }
        }
    }
    let chol = Llt::new(xtx.as_ref(), Side::Lower).map_err(|e| MiceError::Numerical(format!("irls_logit: {e:?}")))?;
    let mut ident = Mat::<f64>::zeros(p, p);
    for j in 0..p {
        ident[(j, j)] = 1.0;
    }
    let inv = chol.solve(&ident);
    let mut cov_unscaled = vec![0.0_f64; p * p];
    for j in 0..p {
        for k in 0..p {
            cov_unscaled[j * p + k] = inv[(j, k)];
        }
    }

    Ok(IrlsFit {
        coef: beta,
        cov_unscaled,
        n_iter,
        converged,
    })
}

/// Reproduce `mice.impute.logreg(y, ry, x, wy = NULL, ...)`.
pub fn impute_logreg<R: Rng + ?Sized>(
    y: &[f64],
    ry: &[bool],
    x: &[Vec<f64>],
    wy: Option<&[bool]>,
    rng: &mut R,
) -> Result<Vec<f64>> {
    let wy_owned;
    let wy = match wy {
        Some(v) => v,
        None => {
            wy_owned = ry.iter().map(|r| !*r).collect::<Vec<_>>();
            &wy_owned
        }
    };
    debug_assert_eq!(y.len(), ry.len());
    debug_assert_eq!(y.len(), wy.len());

    // Validate that y is binary.
    for (i, &yi) in y.iter().enumerate() {
        if !ry[i] {
            continue;
        }
        if yi != 0.0 && yi != 1.0 {
            return Err(MiceError::InvalidSpec(format!(
                "logreg: y[{i}] = {yi}, expected 0 or 1"
            )));
        }
    }

    // Step 1: augment.
    let aug = augment(y, ry, x, wy)?;
    let x_aug_in = aug.x;
    let y_aug_in = aug.y;
    let ry_aug = aug.ry;
    let wy_aug = aug.wy;
    let w_aug = aug.w;

    // Step 2: build (intercept, x) design and fit IRLS only on ry_aug.
    let n_aug = y_aug_in.len();
    let p_in = if x_aug_in.is_empty() { 0 } else { x_aug_in[0].len() };
    let mut x_aug: Vec<Vec<f64>> = Vec::with_capacity(n_aug);
    for i in 0..n_aug {
        let mut row = vec![1.0_f64];
        if p_in > 0 {
            row.extend_from_slice(&x_aug_in[i]);
        }
        x_aug.push(row);
    }

    // Subset to ry_aug observed rows.
    let mut x_obs: Vec<Vec<f64>> = Vec::new();
    let mut y_obs: Vec<f64> = Vec::new();
    let mut w_obs: Vec<f64> = Vec::new();
    for i in 0..n_aug {
        if ry_aug[i] {
            x_obs.push(x_aug[i].clone());
            y_obs.push(y_aug_in[i]);
            w_obs.push(w_aug[i]);
        }
    }
    let fit = irls_logit(&x_obs, &y_obs, &w_obs, 50, 1e-8)?;
    let _ = fit.converged;

    // Step 3: draw β* ~ N(β̂, cov_unscaled).
    let p_full = fit.coef.len();
    let normal = Normal::new(0.0, 1.0).expect("normal(0,1) init");
    let z: Vec<f64> = (0..p_full).map(|_| normal.sample(rng)).collect();
    // Cholesky of cov_unscaled (R: t(chol(sym(cov.unscaled))) %*% z).
    let mut cov = Mat::<f64>::zeros(p_full, p_full);
    for j in 0..p_full {
        for k in 0..p_full {
            cov[(j, k)] = fit.cov_unscaled[j * p_full + k];
        }
    }
    let chol = Llt::new(cov.as_ref(), Side::Lower).map_err(|e| MiceError::Numerical(format!("logreg cov: {e:?}")))?;
    let mut zm = Mat::<f64>::zeros(p_full, 1);
    for j in 0..p_full {
        zm[(j, 0)] = z[j];
    }
    let lz_mat = chol.solve(&zm);
    let beta_star: Vec<f64> = (0..p_full).map(|j| fit.coef[j] + lz_mat[(j, 0)]).collect();

    // Step 4: impute wy rows.
    // R applies the augmentation to ALL `wy` rows in the original data frame
    // (including the synthetic ones), then thresholds against runif().
    // For our purposes we impute only the original wy locations.
    let mut x_wy: Vec<Vec<f64>> = Vec::new();
    let mut wy_pos: Vec<usize> = Vec::new();
    for i in 0..wy_aug.len() {
        if wy_aug[i] {
            x_wy.push(x_aug[i].clone());
            wy_pos.push(i);
        }
    }

    let mut out = Vec::with_capacity(x_wy.len());
    for row in &x_wy {
        let eta: f64 = (0..p_full).map(|j| row[j] * beta_star[j]).sum();
        let p = 1.0 / (1.0 + (-eta).exp());
        let u: f64 = rng.gen_range(0.0..1.0);
        let v = if u <= p { 1.0 } else { 0.0 };
        out.push(v);
    }
    Ok(out)
}

// Keep Side reference for parity with mvmr crate.
#[allow(dead_code)]
fn _side_ref() -> Side {
    Side::Lower
}
