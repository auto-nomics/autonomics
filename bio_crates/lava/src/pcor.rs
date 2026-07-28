//! Partial correlation helpers — faithful port of `R/partial_corrs.R`.
//!
//! Point estimates (partial correlation / covariance / variance) used by
//! `run.pcor`, and `estimate.pcor` for the partial-correlation confidence
//! interval.

use faer::{Mat, MatRef, Side};

/// Solve `A x = b` for a symmetric positive-(semi)definite `A`; returns `None`
/// if not invertible (matching R's `tryCatch(solve(...), → NA)`).
fn solve_spd_opt(a: MatRef<f64>, b: MatRef<f64>) -> Option<Mat<f64>> {
    use faer::linalg::solvers::{Llt, Solve};
    if a.nrows() != a.ncols() {
        return None;
    }
    let llt = Llt::new(a, Side::Lower).ok()?;
    Some(llt.solve(&b.to_owned()))
}

/// `partial.cov(omega, x, y, z)`.
pub fn partial_cov(omega: MatRef<f64>, x: usize, y: usize, z: &[usize]) -> f64 {
    if z.is_empty() {
        return omega[(x, y)];
    }
    let nz = z.len();
    let sub = Mat::from_fn(nz, nz, |i, j| omega[(z[i], z[j])]);
    let bx = Mat::from_fn(nz, 1, |i, _| omega[(z[i], x)]);
    let inv_x = match solve_spd_opt(sub.as_ref(), bx.as_ref()) {
        Some(m) => m,
        None => return f64::NAN,
    };
    let mut dot = 0.0;
    for i in 0..nz {
        dot += inv_x[(i, 0)] * omega[(z[i], y)];
    }
    omega[(x, y)] - dot
}

/// `partial.var(omega, x, z)`.
pub fn partial_var(omega: MatRef<f64>, x: usize, z: &[usize]) -> f64 {
    if z.is_empty() {
        return omega[(x, x)];
    }
    let nz = z.len();
    let sub = Mat::from_fn(nz, nz, |i, j| omega[(z[i], z[j])]);
    let bx = Mat::from_fn(nz, 1, |i, _| omega[(z[i], x)]);
    let inv_x = match solve_spd_opt(sub.as_ref(), bx.as_ref()) {
        Some(m) => m,
        None => return f64::NAN,
    };
    let mut dot = 0.0;
    for i in 0..nz {
        dot += inv_x[(i, 0)] * omega[(z[i], x)];
    }
    omega[(x, x)] - dot
}

/// `partial.cor(omega, x, y, z)`: returns `None` if a partial variance is ≤ 0.
pub fn partial_cor(omega: MatRef<f64>, x: usize, y: usize, z: &[usize]) -> Option<f64> {
    let cov = partial_cov(omega, x, y, z);
    let vx = partial_var(omega, x, z);
    let vy = partial_var(omega, y, z);
    if vx > 0.0 && vy > 0.0 {
        Some(cov / (vx * vy).sqrt())
    } else {
        None
    }
}

/// `estimate.pcor(draw, sigma)`: partial correlation from one Wishart draw,
/// with x = P−2, y = P−1, z = 0..P−2.
pub fn estimate_pcor(draw: MatRef<f64>, sigma: MatRef<f64>) -> f64 {
    let p = sigma.nrows();
    let ix = p - 2;
    let iy = p - 1;
    let iz: Vec<usize> = (0..p - 2).collect();
    let o = Mat::from_fn(p, p, |i, j| draw[(i, j)] - sigma[(i, j)]);
    let cov = partial_cov(o.as_ref(), ix, iy, &iz);
    let vx = partial_var(o.as_ref(), ix, &iz);
    let vy = partial_var(o.as_ref(), iy, &iz);
    if vx > 0.0 && vy > 0.0 {
        cov / (vx * vy).sqrt()
    } else {
        f64::NAN
    }
}
