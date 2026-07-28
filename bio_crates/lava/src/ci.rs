//! Confidence intervals — faithful port of `R/confidence_intervals.R`.
//!
//! Wishart-simulation 95% CIs for the bivariate correlation, multiple
//! regression, and partial correlation. All clamp estimated correlations to
//! just inside (−1, 1) (±0.99999) before sampling, matching R.

use faer::{Mat, MatRef, Side};
use rand::Rng;

use crate::pcor::estimate_pcor;
use crate::stats::cov2cor;
use crate::wishart::WishartCtx;

/// `S = diag(sqrt(diag(omega)))`, then `corrs = S^{-1}·omega·S^{-1}` (cov2cor)
/// with off-diagonal clamped to ±0.99999, then `omega = S·corrs·S`.
fn standardize_omega(omega: &Mat<f64>) -> Mat<f64> {
    let p = omega.nrows();
    let sdiag: Vec<f64> = (0..p).map(|i| omega[(i, i)].max(0.0).sqrt()).collect();
    let mut corrs = Mat::zeros(p, p);
    for i in 0..p {
        for j in 0..p {
            if i == j {
                corrs[(i, j)] = 1.0;
            } else {
                let c = omega[(i, j)] / (sdiag[i] * sdiag[j]);
                let c = if c >= 1.0 {
                    0.99999
                } else if c <= -1.0 {
                    -0.99999
                } else {
                    c
                };
                corrs[(i, j)] = c;
            }
        }
    }
    // omega = S · corrs · S
    let mut out = Mat::zeros(p, p);
    for i in 0..p {
        for j in 0..p {
            let mut v = 0.0;
            for k in 0..p {
                v += sdiag[i] * corrs[(i, k)] * sdiag[k]; // = S·corrs·S with S diagonal
            }
            // S diagonal → (S·corrs·S)[i,j] = sdiag[i]*corrs[i,j]*sdiag[j]
            out[(i, j)] = sdiag[i] * corrs[(i, j)] * sdiag[j];
            let _ = v;
        }
    }
    out
}

/// Quantile via linear interpolation (R type=7), matching `quantile.default`.
fn quantile(sorted: &mut [f64], q: f64) -> f64 {
    if sorted.is_empty() {
        return f64::NAN;
    }
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let n = sorted.len();
    if n == 1 {
        return sorted[0];
    }
    let h = (n - 1) as f64 * q;
    let lo = h.floor() as usize;
    let hi = (lo + 1).min(n - 1);
    let frac = h - lo as f64;
    sorted[lo] * (1.0 - frac) + sorted[hi] * frac
}

/// `ci.bivariate` — returns (rho.lower, rho.upper, r2.lower, r2.upper).
pub fn ci_bivariate<R: Rng>(
    k: usize,
    omega: &Mat<f64>,
    sigma: &Mat<f64>,
    n_iter: usize,
    rng: &mut R,
) -> (f64, f64, f64, f64) {
    let omega = standardize_omega(omega);
    // sigma_used = sigma/K
    let kf = k as f64;
    let sigma_use = Mat::from_fn(2, 2, |i, j| sigma[(i, j)] / kf);
    let ctx = WishartCtx::new(k, sigma_use.as_ref(), omega.as_ref());
    let mut draw = Mat::zeros(2, 2);
    let mut rs: Vec<f64> = Vec::with_capacity(n_iter);
    for _ in 0..n_iter {
        ctx.draw_into(rng, &mut draw);
        // r = cov2cor(draw - sigma)[1,2]
        let diff = Mat::from_fn(2, 2, |i, j| draw[(i, j)] - sigma[(i, j)]);
        let cor = cov2cor(&diff);
        let r = cor[(0, 1)];
        if !r.is_nan() {
            rs.push(r);
        }
    }
    let mut rs2: Vec<f64> = rs.iter().map(|x| x * x).collect();
    let rho_lower = quantile(&mut rs.clone(), 0.025).clamp(-1.0, 1.0);
    let rho_upper = quantile(&mut rs, 0.975).clamp(-1.0, 1.0);
    let mut r2_lower = quantile(&mut rs2.clone(), 0.025).clamp(0.0, 1.0);
    let r2_upper = quantile(&mut rs2, 0.975).clamp(0.0, 1.0);
    // if rho CI spans 0, set r2.lower = 0
    if rho_lower.signum() != rho_upper.signum() {
        r2_lower = 0.0;
    }
    (rho_lower, rho_upper, r2_lower, r2_upper)
}

/// `estimate.std(draw, sigma)` → (gamma_vec length Px, r2).
fn estimate_std(draw: &Mat<f64>, sigma: &Mat<f64>) -> (Vec<f64>, f64) {
    use faer::linalg::solvers::{Llt, Solve};
    let p = sigma.nrows();
    let px = p - 1;
    let o = cov2cor(&Mat::from_fn(p, p, |i, j| draw[(i, j)] - sigma[(i, j)]));
    // g = solve(o[-P,-P]) · o[-P,P]
    let oxx = Mat::from_fn(px, px, |i, j| o[(i, j)]);
    let oxy = Mat::from_fn(px, 1, |i, _| o[(i, p - 1)]);
    let llt = match Llt::new(oxx.as_ref(), Side::Lower) {
        Ok(l) => l,
        Err(_) => return (vec![f64::NAN; px], f64::NAN),
    };
    let g = llt.solve(&oxy);
    // r2 = o[-P,P] · g
    let mut r2 = 0.0;
    for i in 0..px {
        r2 += oxy[(i, 0)] * g[(i, 0)];
    }
    let gv: Vec<f64> = (0..px).map(|i| g[(i, 0)]).collect();
    (gv, r2)
}

/// `ci.multivariate` — returns (gamma.lower Vec, gamma.upper Vec, r2.lower, r2.upper).
pub fn ci_multivariate<R: Rng>(
    k: usize,
    omega: &Mat<f64>,
    sigma: &Mat<f64>,
    n_iter: usize,
    rng: &mut R,
) -> (Vec<f64>, Vec<f64>, f64, f64) {
    let p = omega.nrows();
    let px = p - 1;
    let omega = standardize_omega(omega);
    let kf = k as f64;
    let sigma_use = Mat::from_fn(p, p, |i, j| sigma[(i, j)] / kf);
    let ctx = WishartCtx::new(k, sigma_use.as_ref(), omega.as_ref());
    let mut draw = Mat::zeros(p, p);
    let mut samples: Vec<(Vec<f64>, f64)> = Vec::with_capacity(n_iter);
    for _ in 0..n_iter {
        ctx.draw_into(rng, &mut draw);
        samples.push(estimate_std(&draw, sigma));
    }
    let mut gamma_lower = vec![f64::NAN; px];
    let mut gamma_upper = vec![f64::NAN; px];
    for j in 0..px {
        let mut col: Vec<f64> = samples
            .iter()
            .map(|(g, _)| g[j])
            .filter(|x| !x.is_nan())
            .collect();
        gamma_lower[j] = quantile(&mut col.clone(), 0.025);
        gamma_upper[j] = quantile(&mut col, 0.975);
    }
    let mut r2s: Vec<f64> = samples
        .iter()
        .map(|(_, r)| *r)
        .filter(|x| !x.is_nan())
        .collect();
    let r2_lower = quantile(&mut r2s.clone(), 0.025).clamp(0.0, 1.0);
    let r2_upper = quantile(&mut r2s, 0.975).clamp(0.0, 1.0);
    (gamma_lower, gamma_upper, r2_lower, r2_upper)
}

/// `ci.pcor` — returns (estimate, ci.lower, ci.high).
pub fn ci_pcor<R: Rng>(
    k: usize,
    xy_index: (usize, usize),
    z_index: &[usize],
    omega: &Mat<f64>,
    sigma: &Mat<f64>,
    n_iter: usize,
    rng: &mut R,
) -> (f64, f64, f64) {
    let _pfull = omega.nrows();
    // reorder: z first, then xy at end (x=P-2, y=P-1)
    let mut idx: Vec<usize> = z_index.to_vec();
    idx.push(xy_index.0);
    idx.push(xy_index.1);
    let omega_r = submatrix(omega, &idx);
    let sigma_r = submatrix(sigma, &idx);
    let omega_r = standardize_omega(&omega_r);
    let p = omega_r.nrows(); // submatrix size = |z| + 2

    // point estimate (reference)
    let pcov =
        omega_r[(p - 2, p - 1)] - fit(&omega_r, p - 2, p - 1, &(0..p - 2).collect::<Vec<_>>());
    let _ = pcov;
    let est = estimate_pcor_point(&omega_r, p);

    let kf = k as f64;
    let sigma_use = Mat::from_fn(p, p, |i, j| sigma_r[(i, j)] / kf);
    let ctx = WishartCtx::new(k, sigma_use.as_ref(), omega_r.as_ref());
    let mut draw = Mat::zeros(p, p);
    let mut vals: Vec<f64> = Vec::with_capacity(n_iter);
    for _ in 0..n_iter {
        ctx.draw_into(rng, &mut draw);
        let v = estimate_pcor(draw.as_ref(), sigma_r.as_ref());
        if !v.is_nan() {
            vals.push(v);
        }
    }
    let ci_lo = quantile(&mut vals.clone(), 0.025).clamp(-1.0, 1.0);
    let ci_hi = quantile(&mut vals, 0.975).clamp(-1.0, 1.0);
    (est, ci_lo, ci_hi)
}

fn fit(omega: &Mat<f64>, x: usize, y: usize, z: &[usize]) -> f64 {
    crate::pcor::partial_cov(omega.as_ref(), x, y, z)
}

fn estimate_pcor_point(omega: &Mat<f64>, p: usize) -> f64 {
    let z: Vec<usize> = (0..p - 2).collect();
    crate::pcor::partial_cor(omega.as_ref(), p - 2, p - 1, &z).unwrap_or(f64::NAN)
}

fn submatrix(m: &Mat<f64>, idx: &[usize]) -> Mat<f64> {
    let n = idx.len();
    Mat::from_fn(n, n, |i, j| m[(idx[i], idx[j])])
}
