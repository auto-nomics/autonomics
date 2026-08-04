//! Weak-instrument-adjusted MVMR via Q-minimisation — faithful port of
//! `R/qhet_mvmr.R`.
//!
//! The estimator minimises a profiled Cochran's Q statistic in two layers:
//!
//! 1. **Inner** — for fixed overdispersion `τ²`, minimise
//!    `Q(β; τ²) = Σ_l (γ_l − Π_l·β)² / (se(γ_l)² + βᵀ Σ_l β + τ²)`
//!    over `β`, where `Σ_l = pcor · diag(se_X,l) · diag(se_X,l)` is the
//!    per-SNP exposure-effect covariance implied by the phenotypic correlation
//!    matrix. R solves this with `optim()` (Nelder-Mead).
//! 2. **Outer** — find `τ² ∈ [−10, 10]` minimising
//!    `(Q(β̂(τ²)) − (n − 2))²`. R solves this with `optimize()` (Brent's
//!    golden-section search).
//!
//! The returned point estimates are `β̂(τ̂²)`. As in the R reference, this port
//! does **not** reproduce the bootstrap confidence intervals (the `boot::boot`
//! RNG stream cannot be shared between R and Rust); callers requiring CIs must
//! wrap [`qhet_mvmr`] in their own resampling loop.

use crate::error::{MvmrError, Result};
use crate::format::MvmrInput;

/// Output of [`qhet_mvmr`].
#[derive(Debug, Clone)]
pub struct QhetResult {
    /// Per-exposure effect estimate (length `p`).
    pub estimate: Vec<f64>,
    /// Estimated overdispersion `τ̂²`.
    pub tau2: f64,
    /// Final minimised Q statistic at `(β̂, τ̂²)`.
    pub q_stat: f64,
}

/// Per-SNP exposure-effect covariance matrices built from a phenotypic
/// correlation matrix (`pcor`) and the per-exposure standard errors, matching
/// R's `covlist <- lapply(... function(l) correlation * outer(stderr[l,], stderr[l,]))`.
pub fn covlist_from_pcor(input: &MvmrInput, pcor: &[Vec<f64>]) -> Vec<Vec<Vec<f64>>> {
    let n = input.n_snps();
    let p = input.n_exposures();
    let sebetas = input.sebeta_xg_matrix();
    let mut covlist = vec![vec![vec![0.0; p]; p]; n];
    for l in 0..n {
        for a in 0..p {
            for b in 0..p {
                covlist[l][a][b] = pcor[a][b] * sebetas[l][a] * sebetas[l][b];
            }
        }
    }
    covlist
}

/// Inner objective: for fixed `τ²`, return the minimised Q and the argmin β.
///
/// Uses Newton's method with analytical gradient and numerical Hessian, which
/// converges to the same smooth minimum as R's `optim(Nelder-Mead)` for
/// well-identified problems. Falls back to Nelder-Mead if the Hessian is
/// non-PD.
fn inner_min_q(
    gamma: &[f64],
    segamma: &[f64],
    pihat: &[Vec<f64>],
    covlist: &[Vec<Vec<f64>>],
    tau2: f64,
    p: usize,
) -> (f64, Vec<f64>) {
    let n = gamma.len();

    // Objective Q(β) = Σ_l (γ_l − Π_l·β)² / w_l(β)
    let objective = |beta: &[f64]| -> f64 {
        let mut q = 0.0;
        for l in 0..n {
            let bsb = quad_form(beta, &covlist[l], p);
            let w = segamma[l] * segamma[l] + bsb + tau2;
            let resid = gamma[l] - dot(&pihat[l], beta);
            q += resid * resid / w;
        }
        q
    };

    // Analytical gradient.
    let gradient = |beta: &[f64]| -> Vec<f64> {
        let mut grad = vec![0.0; p];
        for l in 0..n {
            let sigma_beta = mv(&covlist[l], beta, p); // Σ_l · β
            let bsb = dot(beta, &sigma_beta);
            let w = segamma[l] * segamma[l] + bsb + tau2;
            let resid = gamma[l] - dot(&pihat[l], beta);
            // ∂Q/∂β_k = −2 Π_{lk} resid / w − resid² · 2 (Σ_l β)_k / w²
            for k in 0..p {
                grad[k] += -2.0 * pihat[l][k] * resid / w
                    - resid * resid * 2.0 * sigma_beta[k] / (w * w);
            }
        }
        grad
    };

    // Newton's method with numerical Hessian.
    let mut beta = vec![0.0; p];
    let h = 1e-6;
    for _iter in 0..200 {
        let grad = gradient(&beta);
        let gnorm: f64 = grad.iter().map(|g| g.abs()).fold(0.0_f64, f64::max);
        if gnorm < 1e-14 {
            break;
        }
        // Numerical Hessian.
        let mut hess = vec![vec![0.0; p]; p];
        for a in 0..p {
            for b_idx in a..p {
                let mut beta_pp = beta.clone();
                let mut beta_pm = beta.clone();
                let mut beta_mp = beta.clone();
                let mut beta_mm = beta.clone();
                beta_pp[a] += h;
                beta_pp[b_idx] += h;
                beta_pm[a] += h;
                beta_pm[b_idx] -= h;
                beta_mp[a] -= h;
                beta_mp[b_idx] += h;
                beta_mm[a] -= h;
                beta_mm[b_idx] -= h;
                hess[a][b_idx] = (objective(&beta_pp) - objective(&beta_pm)
                    - objective(&beta_mp)
                    + objective(&beta_mm))
                    / (4.0 * h * h);
                hess[b_idx][a] = hess[a][b_idx];
            }
        }
        // Solve Hessian · step = −gradient via Cholesky (faer).
        let mat = faer::Mat::from_fn(p, p, |i, j| hess[j][i]);
        let rhs = faer::Mat::from_fn(p, 1, |i, _| -grad[i]);
        use faer::linalg::solvers::{DenseSolveCore, Llt, Solve};
        use faer::Side;
        let step = match Llt::new(mat.as_ref(), Side::Lower) {
            Ok(llt) => llt.solve(&rhs),
            Err(_) => {
                // Hessian not PD — fall back to Nelder-Mead.
                let reltol = f64::EPSILON.sqrt();
                let (nm_beta, nm_q) = nelder_mead(&beta, &objective, reltol, 5000);
                return (nm_q, nm_beta);
            }
        };
        let new_beta: Vec<f64> = (0..p).map(|i| beta[i] + step[(i, 0)]).collect();
        let max_change: f64 = new_beta
            .iter()
            .zip(&beta)
            .map(|(a, b)| (a - b).abs())
            .fold(0.0_f64, f64::max);
        beta = new_beta;
        if max_change < 1e-14 {
            break;
        }
    }
    let q = objective(&beta);
    (q, beta)
}

fn dot(a: &[f64], b: &[f64]) -> f64 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

fn mv(mat: &[Vec<f64>], v: &[f64], p: usize) -> Vec<f64> {
    let mut out = vec![0.0; p];
    for i in 0..p {
        for j in 0..p {
            out[i] += mat[i][j] * v[j];
        }
    }
    out
}

fn quad_form(v: &[f64], mat: &[Vec<f64>], p: usize) -> f64 {
    let mut s = 0.0;
    for a in 0..p {
        for b in 0..p {
            s += v[a] * mat[a][b] * v[b];
        }
    }
    s
}

/// Fit the weak-instrument-adjusted MVMR model.
///
/// `pcor` is the `p × p` phenotypic correlation matrix between exposures.
pub fn qhet_mvmr(input: &MvmrInput, pcor: &[Vec<f64>]) -> Result<QhetResult> {
    input.validate()?;
    let n = input.n_snps();
    let p = input.n_exposures();
    if pcor.len() != p || pcor.iter().any(|r| r.len() != p) {
        return Err(MvmrError::LengthMismatch(format!(
            "pcor must be {p}×{p}"
        )));
    }

    let gamma = &input.beta_yg;
    let segamma = &input.sebeta_yg;
    let pihat = input.beta_xg_matrix(); // n × p
    let covlist = covlist_from_pcor(input, pcor);

    // ── Outer: minimise (Q(β̂(τ²)) − (n − 2))² over τ² ∈ [−10, 10] ──
    let target = (n as f64) - 2.0;
    let outer = |tau2: f64| -> f64 {
        let (q, _) = inner_min_q(gamma, segamma, &pihat, &covlist, tau2, p);
        (q - target).powi(2)
    };
    let (tau2, _) = golden_section(&outer, -10.0, 10.0, 1e-12, 2000);

    // ── Re-optimise β at τ̂² to obtain the reported effect estimates ──
    let (q_stat, estimate) = inner_min_q(gamma, segamma, &pihat, &covlist, tau2, p);

    Ok(QhetResult {
        estimate,
        tau2,
        q_stat,
    })
}

// ── optimisers ─────────────────────────────────────────────────────────────

/// Nelder-Mead simplex search, matching R's `optim(method = "Nelder-Mead")`
/// closely enough that smooth, well-identified objectives converge to the same
/// minimum. Returns `(argmin, f(argmin))`.
fn nelder_mead<F: Fn(&[f64]) -> f64>(
    start: &[f64],
    f: &F,
    reltol: f64,
    max_iter: usize,
) -> (Vec<f64>, f64) {
    let n = start.len();
    let mut simplex: Vec<Vec<f64>> = Vec::with_capacity(n + 1);
    let mut fvals: Vec<f64> = Vec::with_capacity(n + 1);
    simplex.push(start.to_vec());
    fvals.push(f(start));
    for i in 0..n {
        let mut v = start.to_vec();
        // R builds the initial simplex with step 0.05 * xi (or 0.00025 if zero).
        let step = if v[i].abs() > 1e-12 {
            0.05 * v[i]
        } else {
            0.00025
        };
        v[i] += step;
        fvals.push(f(&v));
        simplex.push(v);
    }

    let alpha = 1.0; // reflection
    let gamma = 2.0; // expansion
    let beta = 0.5; // contraction

    let mut iter = 0;
    while iter < max_iter {
        // Find best (lowest) and worst (highest) vertices.
        let mut best = 0;
        let mut worst = 0;
        for i in 1..=n {
            if fvals[i] < fvals[best] {
                best = i;
            }
            if fvals[i] > fvals[worst] {
                worst = i;
            }
        }
        // second_worst = the highest excluding worst
        let mut second_worst = best;
        for i in 0..=n {
            if i == worst {
                continue;
            }
            if fvals[i] > fvals[second_worst] {
                second_worst = i;
            }
        }

        // R's convergence: |f_best − f_worst| < reltol * |f_best| + abstol.
        let abstol = 0.0;
        if (fvals[best] - fvals[worst]).abs() <= reltol * fvals[best].abs() + abstol {
            break;
        }

        // Centroid of all but the worst.
        let mut centroid = vec![0.0; n];
        for i in 0..=n {
            if i == worst {
                continue;
            }
            for d in 0..n {
                centroid[d] += simplex[i][d];
            }
        }
        for d in 0..n {
            centroid[d] /= n as f64;
        }

        // Reflection.
        let xr: Vec<f64> = (0..n)
            .map(|d| centroid[d] + alpha * (centroid[d] - simplex[worst][d]))
            .collect();
        let fr = f(&xr);
        if fr >= fvals[best] && fr < fvals[second_worst] {
            simplex[worst] = xr;
            fvals[worst] = fr;
        } else if fr < fvals[best] {
            // Expansion.
            let xe: Vec<f64> = (0..n)
                .map(|d| centroid[d] + gamma * (xr[d] - centroid[d]))
                .collect();
            let fe = f(&xe);
            if fe < fr {
                simplex[worst] = xe;
                fvals[worst] = fe;
            } else {
                simplex[worst] = xr;
                fvals[worst] = fr;
            }
        } else {
            // Contraction.
            let xc: Vec<f64> = (0..n)
                .map(|d| centroid[d] + beta * (simplex[worst][d] - centroid[d]))
                .collect();
            let fc = f(&xc);
            if fc < fvals[worst] {
                simplex[worst] = xc;
                fvals[worst] = fc;
            } else {
                // Shrink toward best.
                let best_v = simplex[best].clone();
                for i in 0..=n {
                    if i == best {
                        continue;
                    }
                    for d in 0..n {
                        simplex[i][d] = best_v[d] + 0.5 * (simplex[i][d] - best_v[d]);
                    }
                    fvals[i] = f(&simplex[i]);
                }
            }
        }
        iter += 1;
    }

    let mut best = 0;
    for i in 1..=n {
        if fvals[i] < fvals[best] {
            best = i;
        }
    }
    (simplex[best].clone(), fvals[best])
}

/// Coarse grid scan + golden-section refinement on `[a, b]`, matching R's
/// `optimize()` for smooth one-dimensional objectives. Returns
/// `(argmin, f(argmin))`.
fn golden_section<F: Fn(f64) -> f64>(
    f: &F,
    a: f64,
    b: f64,
    tol: f64,
    max_iter: usize,
) -> (f64, f64) {
    // Phase 1: coarse grid scan to bracket the minimum.
    let n_grid = 201;
    let step = (b - a) / (n_grid - 1) as f64;
    let mut best_x = a;
    let mut best_f = f(a);
    for i in 1..n_grid {
        let x = a + i as f64 * step;
        let fx = f(x);
        if fx < best_f {
            best_f = fx;
            best_x = x;
        }
    }
    // Refine the bracket around the best grid point.
    let mut lo = (best_x - step).max(a);
    let mut hi = (best_x + step).min(b);

    // Phase 2: golden section within [lo, hi].
    let gr = (5.0_f64.sqrt() - 1.0) / 2.0; // ≈ 0.618
    let mut c = hi - gr * (hi - lo);
    let mut d = lo + gr * (hi - lo);
    let mut fc = f(c);
    let mut fd = f(d);
    for _ in 0..max_iter {
        if (hi - lo).abs() < tol {
            break;
        }
        if fc < fd {
            hi = d;
            d = c;
            fd = fc;
            c = hi - gr * (hi - lo);
            fc = f(c);
        } else {
            lo = c;
            c = d;
            fc = fd;
            d = lo + gr * (hi - lo);
            fd = f(d);
        }
    }
    if fc < fd {
        (c, fc)
    } else {
        (d, fd)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn golden_section_finds_minimum() {
        // f(x) = (x − 2)^2, minimum at x = 2.
        let (x, fx) = golden_section(&|x: f64| (x - 2.0).powi(2), -10.0, 10.0, 1e-12, 200);
        assert!((x - 2.0).abs() < 1e-6);
        assert!(fx.abs() < 1e-12);
    }

    #[test]
    fn nelder_mead_finds_quadratic_minimum() {
        // f(x,y) = (x−1)^2 + (y−2)^2.
        let (v, f) = nelder_mead(&[0.0, 0.0], &|x: &[f64]| (x[0] - 1.0).powi(2) + (x[1] - 2.0).powi(2), 1e-12, 500);
        assert!((v[0] - 1.0).abs() < 1e-6);
        assert!((v[1] - 2.0).abs() < 1e-6);
        assert!(f.abs() < 1e-12);
    }
}
