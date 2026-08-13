//! Generalized gradient descent — port of `ggdescent()` and `ggstep()` in `hierNet.c`.
//!
//! Solves the proximal subproblem: min Loss + Penalty,
//! using backtracking line search and the proximal operator from `prox.rs`.

use super::HierNetCoefs;
use super::interactions::{compute_dot_grad_del, compute_phat, compute_yhat, cross_prod};
use super::prox;

/// Generalized gradient descent for Gaussian loss.
///
/// Port of `ggdescent()` in `hierNet.c`.
pub fn ggdescent(
    x: &[f64],
    n: usize,
    p: usize,
    zz: &[f64],
    diagonal: bool,
    y: &[f64],
    lam_l1: f64,
    lam_l2: f64,
    rho: f64,
    v: &[f64],
    step: f64,
    backtrack: f64,
    maxiter: usize,
    tol: f64,
    init: &HierNetCoefs,
) -> crate::Result<HierNetCoefs> {
    let stepwindow = 10usize;
    let mut cur = init.clone();
    let mut best = init.clone();

    let mut ttt = vec![step; stepwindow];

    for l in 0..maxiter {
        // Determine step size to try
        let tt = if l < stepwindow {
            step
        } else {
            ttt.iter().fold(0.0f64, |a, &b| a.max(b))
        };

        let (ttaken, maxabsdel) = ggstep(
            x, n, p, zz, diagonal, y, lam_l1, lam_l2, rho, v, &cur, tt, backtrack, &mut best,
        );

        ttt[l % stepwindow] = ttaken;

        if maxabsdel < tol {
            break;
        }

        // Swap cur ← best for next iteration
        std::mem::swap(&mut cur, &mut best);
    }

    Ok(cur)
}

/// Single ggstep with backtracking line search (Gaussian).
///
/// Port of `ggstep()` in `hierNet.c`.
fn ggstep(
    x: &[f64],
    n: usize,
    p: usize,
    zz: &[f64],
    diagonal: bool,
    y: &[f64],
    lam_l1: f64,
    lam_l2: f64,
    rho: f64,
    v: &[f64],
    cur: &HierNetCoefs,
    t_init: f64,
    backtrack: f64,
    result: &mut HierNetCoefs,
) -> (f64, f64) {
    // Compute current residual and right-hand-side constant
    let curr_yhat = compute_yhat(x, n, p, zz, diagonal, &cur.th, &cur.bp, &cur.bn);
    let mut curr = vec![0.0; n];
    let mut right0 = 0.0;
    for i in 0..n {
        curr[i] = y[i] - curr_yhat[i];
        right0 += curr[i] * curr[i];
    }
    right0 /= 2.0;

    let cm = cross_prod(x, n, p, &curr);

    let mut tt = t_init;
    let small = 1e-80;

    while tt > small {
        // Take a proximal step
        let (th, bp, bn) = prox::prox_zz_given_r(
            x, n, p, zz, diagonal, &curr, lam_l1, lam_l2, rho, v, &cur.th, &cur.bp, &cur.bn, tt,
        );

        // Compute new residual and left (loss)
        let new_yhat = compute_yhat(x, n, p, zz, diagonal, &th, &bp, &bn);
        let mut left = 0.0;
        let mut r_new = vec![0.0; n];
        for i in 0..n {
            r_new[i] = y[i] - new_yhat[i];
            left += r_new[i] * r_new[i];
        }
        left /= 2.0;

        // Compute right = right0 + grad·delta + delta·delta / (2t)
        let mut right = right0;
        // dot product of gradient with delta for main effects
        let mut delta_sq_norm = 0.0;
        for j in 0..p {
            let delbp = bp[j] - cur.bp[j];
            let delbn = bn[j] - cur.bn[j];
            right += cm[j] * (delbn - delbp);
            delta_sq_norm += delbp * delbp + delbn * delbn;
        }
        // Interaction delta
        let delth: Vec<f64> = (0..p * p).map(|jj| th[jj] - cur.th[jj]).collect();
        let dotprod = compute_dot_grad_del(zz, diagonal, n, p, &curr, &delth);
        right += dotprod;
        for jj in 0..p * p {
            delta_sq_norm += delth[jj] * delth[jj];
        }
        right += delta_sq_norm / (2.0 * tt);

        if left <= right {
            // Accept step
            result.th = th;
            result.bp = bp;
            result.bn = bn;
            result.b0 = cur.b0;

            // Compute max absolute change
            let mut maxabsdel = 0.0f64;
            for j in 0..p {
                maxabsdel = maxabsdel.max((result.bp[j] - cur.bp[j]).abs());
                maxabsdel = maxabsdel.max((result.bn[j] - cur.bn[j]).abs());
            }
            for jj in 0..p * p {
                maxabsdel = maxabsdel.max((result.th[jj] - cur.th[jj]).abs());
            }
            return (tt, maxabsdel);
        }

        tt *= backtrack;
    }

    // Fallback: keep current
    result.th = cur.th.clone();
    result.bp = cur.bp.clone();
    result.bn = cur.bn.clone();
    result.b0 = cur.b0;
    (tt, 0.0)
}

/// Generalized gradient descent for logistic loss.
pub fn ggdescent_logistic(
    x: &[f64],
    n: usize,
    p: usize,
    zz: &[f64],
    diagonal: bool,
    y: &[f64],
    lam_l1: f64,
    lam_l2: f64,
    rho: f64,
    v: &[f64],
    step: f64,
    backtrack: f64,
    maxiter: usize,
    tol: f64,
    init: &HierNetCoefs,
) -> crate::Result<HierNetCoefs> {
    let stepwindow = 10usize;
    let mut cur = init.clone();
    let mut best = init.clone();
    let mut ttt = vec![step; stepwindow];

    for l in 0..maxiter {
        let tt = if l < stepwindow {
            step
        } else {
            ttt.iter().fold(0.0f64, |a, &b| a.max(b))
        };

        let (ttaken, maxabsdel) = ggstep_logistic(
            x, n, p, zz, diagonal, y, lam_l1, lam_l2, rho, v, &cur, tt, backtrack, &mut best,
        );

        ttt[l % stepwindow] = ttaken;

        if maxabsdel < tol {
            break;
        }

        std::mem::swap(&mut cur, &mut best);
    }

    Ok(cur)
}

/// Logistic ggstep.
fn ggstep_logistic(
    x: &[f64],
    n: usize,
    p: usize,
    zz: &[f64],
    diagonal: bool,
    y: &[f64],
    lam_l1: f64,
    lam_l2: f64,
    rho: f64,
    v: &[f64],
    cur: &HierNetCoefs,
    t_init: f64,
    backtrack: f64,
    result: &mut HierNetCoefs,
) -> (f64, f64) {
    // Current residual: y - phat
    let phat = compute_phat(x, n, p, zz, diagonal, cur.b0, &cur.th, &cur.bp, &cur.bn);
    let mut curr = vec![0.0; n];
    let mut sum_curr = 0.0;
    for i in 0..n {
        curr[i] = y[i] - phat[i];
        sum_curr += curr[i];
    }

    // Current loss
    let mut curloss = 0.0;
    let cur_yhat = compute_yhat(x, n, p, zz, diagonal, &cur.th, &cur.bp, &cur.bn);
    for i in 0..n {
        curloss += (1.0 + (-(2.0 * y[i] - 1.0) * (cur.b0 + cur_yhat[i])).exp()).ln();
    }

    let mut tt = t_init;
    let small = 1e-80;

    while tt > small {
        // Update intercept: b0 = curb0 + t * sum_curr
        let new_b0 = cur.b0 + tt * sum_curr;

        let (th, bp, bn) = prox::prox_zz_given_r_logistic(
            x, n, p, zz, diagonal, &curr, lam_l1, lam_l2, rho, v, &cur.th, &cur.bp, &cur.bn, tt,
        );

        // Compute new loss
        let new_yhat = compute_yhat(x, n, p, zz, diagonal, &th, &bp, &bn);
        let mut loss = 0.0;
        for i in 0..n {
            loss += (1.0 + (-(2.0 * y[i] - 1.0) * (new_b0 + new_yhat[i])).exp()).ln();
        }

        // Check majorization condition
        let delb0 = new_b0 - cur.b0;
        let mut sqnorm = delb0 * delb0;
        for j in 0..p {
            let delbp = bp[j] - cur.bp[j];
            let delbn = bn[j] - cur.bn[j];
            sqnorm += delbp * delbp + delbn * delbn;
        }
        let delth: Vec<f64> = (0..p * p).map(|jj| th[jj] - cur.th[jj]).collect();
        for jj in 0..p * p {
            sqnorm += delth[jj] * delth[jj];
        }
        sqnorm /= 2.0 * tt;

        let delyhat = compute_yhat(
            x,
            n,
            p,
            zz,
            diagonal,
            &delth,
            &bp_minus(&cur.bp, &bp),
            &bp_minus(&cur.bn, &bn),
        );
        let mut right = curloss + sqnorm;
        for i in 0..n {
            right -= (delyhat[i] + delb0) * curr[i];
        }

        if loss <= right + 1e-10 {
            result.th = th;
            result.bp = bp;
            result.bn = bn;
            result.b0 = new_b0;

            let mut maxabsdel = delb0.abs();
            for j in 0..p {
                maxabsdel = maxabsdel.max((result.bp[j] - cur.bp[j]).abs());
                maxabsdel = maxabsdel.max((result.bn[j] - cur.bn[j]).abs());
            }
            for jj in 0..p * p {
                maxabsdel = maxabsdel.max((result.th[jj] - cur.th[jj]).abs());
            }
            return (tt, maxabsdel);
        }

        tt *= backtrack;
    }

    result.th = cur.th.clone();
    result.bp = cur.bp.clone();
    result.bn = cur.bn.clone();
    result.b0 = cur.b0;
    (tt, 0.0)
}

/// Helper: compute delta bp = new - old
fn bp_minus(old: &[f64], new: &[f64]) -> Vec<f64> {
    new.iter().zip(old).map(|(&n, &o)| n - o).collect()
}
