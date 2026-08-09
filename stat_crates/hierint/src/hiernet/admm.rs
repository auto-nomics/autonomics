//! ADMM4 for strong hierarchy — port of `admm4()` in `funcs.R`.
//!
//! Alternating Direction Method of Multipliers wrapper that enforces
//! symmetry (th = th^T) and hierarchy constraint (||th_j||_1 ≤ bp[j] + bn[j]).

use super::ggdescent;
use super::interactions::compute_yhat;
use super::HierNetCoefs;

/// ADMM4 for Gaussian loss with strong hierarchy.
pub fn admm4(
    x: &[f64],
    zz: &[f64],
    y: &[f64],
    lam_l1: f64,
    lam_l2: f64,
    diagonal: bool,
    rho: f64,
    niter: usize,
    _sym_eps: f64,
    step: f64,
    maxiter: usize,
    backtrack: f64,
    tol: f64,
    init: &HierNetCoefs,
) -> crate::Result<HierNetCoefs> {
    let n = y.len();
    let p = init.bp.len();

    // ADMM state
    let mut aa = init.clone();
    let mut tt = symmetrize(&aa.th, p); // tt = (th + th^T) / 2
    let mut u = vec![0.0; p * p];

    for _ in 0..niter {
        // V = u - rho * tt
        let v: Vec<f64> = (0..p * p).map(|jj| u[jj] - rho * tt[jj]).collect();

        // Inner: generalized gradient descent
        aa = ggdescent::ggdescent(
            x, n, p, zz, diagonal, y, lam_l1, lam_l2, rho, &v,
            step, backtrack, maxiter, tol, &aa,
        )?;

        // Update tt: tt = (th + th^T)/2 + (u + u^T)/(2*rho)
        for j in 0..p {
            for k in 0..p {
                let idx = j + p * k;
                tt[idx] = (aa.th[idx] + aa.th[k + p * j]) / 2.0
                    + (u[idx] + u[k + p * j]) / (2.0 * rho);
            }
        }

        // Update u: u += rho * (th - tt)
        for jj in 0..p * p {
            u[jj] += rho * (aa.th[jj] - tt[jj]);
        }
    }

    // Symmetrize final th
    aa.th = symmetrize(&aa.th, p);

    // Enforce hierarchy: zero out small violations
    let ii: Vec<bool> = (0..p).map(|j| aa.bp[j] + aa.bn[j] == 0.0).collect();
    let sum_ii = ii.iter().filter(|&&b| b).count();
    if sum_ii > 0 && sum_ii < p {
        let mut max_violation = 0.0f64;
        for j in 0..p {
            if !ii[j] {
                for k in 0..p {
                    if ii[k] {
                        max_violation = max_violation.max(aa.th[j + p * k].abs());
                    }
                }
            }
        }
        if max_violation > 0.0 {
            for jj in 0..p * p {
                if aa.th[jj].abs() <= max_violation {
                    aa.th[jj] = 0.0;
                }
            }
        }
    }

    Ok(aa)
}

/// ADMM4 for logistic loss.
pub fn admm4_logistic(
    x: &[f64],
    zz: &[f64],
    y: &[f64],
    lam_l1: f64,
    lam_l2: f64,
    diagonal: bool,
    rho: f64,
    niter: usize,
    _sym_eps: f64,
    step: f64,
    maxiter: usize,
    backtrack: f64,
    tol: f64,
    init: &HierNetCoefs,
) -> crate::Result<HierNetCoefs> {
    let n = y.len();
    let p = init.bp.len();

    let mut aa = init.clone();
    let mut tt = symmetrize(&aa.th, p);
    let mut u = vec![0.0; p * p];

    for _ in 0..niter {
        let v: Vec<f64> = (0..p * p).map(|jj| u[jj] - rho * tt[jj]).collect();

        aa = ggdescent::ggdescent_logistic(
            x, n, p, zz, diagonal, y, lam_l1, lam_l2, rho, &v,
            step, backtrack, maxiter, tol, &aa,
        )?;

        for j in 0..p {
            for k in 0..p {
                let idx = j + p * k;
                tt[idx] = (aa.th[idx] + aa.th[k + p * j]) / 2.0
                    + (u[idx] + u[k + p * j]) / (2.0 * rho);
            }
        }

        for jj in 0..p * p {
            u[jj] += rho * (aa.th[jj] - tt[jj]);
        }
    }

    aa.th = symmetrize(&aa.th, p);
    Ok(aa)
}

/// Compute (th + th^T) / 2.
fn symmetrize(th: &[f64], p: usize) -> Vec<f64> {
    let mut s = vec![0.0; p * p];
    for j in 0..p {
        for k in 0..p {
            s[j + p * k] = (th[j + p * k] + th[k + p * j]) / 2.0;
        }
    }
    s
}
