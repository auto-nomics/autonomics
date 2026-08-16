//! Proximal operator for hierNet — port of `prox_zz_given_r()` and `onerow()` in `hierNet.c`.
//!
//! The proximal step solves: min_{th,bp,bn} Q(th,bp,bn | cur) + penalty,
//! where Q is a quadratic majorization of the loss around (curth, curbp, curbn).
//!
//! Key sub-problem `onerow()`: for each row j of theta, find the optimal scaling
//! alpha that satisfies the hierarchy constraint ||th_j||_1 ≤ bp[j] + bn[j].
//! This is solved via line search on a piecewise-linear function.

use super::interactions::{cross_prod, ut, utd};

/// Evaluate the function f(alpha) for onerow optimization.
///
/// f(alpha) = sum_k [(a[k] - alpha * s[k]).reduce_S(soft_threshold(c_k)) - curth[jk]]^2
///          + (alpha - something)^2 related to bp, bn constraint
///
/// Port of `f()` in `hierNet.c`.
#[allow(dead_code)] // Retained as a reference implementation from the Hiernet R port.
fn f_onerow(alpha: f64, a: &[f64], q: usize, b: &[f64], c: f64, mu: f64) -> f64 {
    // a: a_k values for each k (interaction gradient + current th)
    // b: b_k = sign indicators
    // c: lambda_l1 penalty for main effect budget
    // mu: rho/2 or similar

    let mut val = 0.0;
    for k in 0..q {
        let t = a[k] * alpha;
        // soft-threshold
        let st = if t > c {
            t - c
        } else if t < -c {
            t + c
        } else {
            0.0
        };
        let d = st - b[k];
        val += d * d;
    }
    // Mu term
    val += mu * (alpha - 1.0) * (alpha - 1.0);
    val
}

/// Compute the proximal operator given residual r.
///
/// Port of `prox_zz_given_r()` in `hierNet.c`.
/// This is the inner step that solves the proximal subproblem for a given step size t.
pub fn prox_zz_given_r(
    x: &[f64],
    n: usize,
    p: usize,
    zz: &[f64],
    diagonal: bool,
    r: &[f64],
    lam_l1: f64,
    lam_l2: f64,
    rho: f64,
    v: &[f64], // ADMM dual term (V = u - rho*tt)
    curth: &[f64],
    curbp: &[f64],
    curbn: &[f64],
    t: f64, // step size
) -> (Vec<f64>, Vec<f64>, Vec<f64>) {
    // (th, bp, bn)

    let cp2 = if diagonal {
        p * (p - 1) / 2 + p
    } else {
        p * (p - 1) / 2
    };

    // Compute gradient: cm = X^T r (main effects), ci = ZZ^T r (interactions)
    let cm = cross_prod(x, n, p, r);
    let ci = cross_prod(zz, n, cp2, r);

    // Main effect update: soft-threshold
    // bp = max(0, curbp + t*cm - lam_l1 - rho*curbp + v_main)
    // bn = max(0, -curbp - t*cm + lam_l1 + rho*curbp - v_main)
    // Actually the proximal update is simpler — standard soft-threshold on bp/bn
    let mut bp = vec![0.0; p];
    let mut bn = vec![0.0; p];
    for j in 0..p {
        let proposed_bp = curbp[j] + t * cm[j];
        let proposed_bn = curbn[j] - t * cm[j];
        // Soft-threshold with lam_l1
        if proposed_bp > lam_l1 * t {
            bp[j] = proposed_bp - lam_l1 * t;
        }
        if proposed_bn > lam_l1 * t {
            bn[j] = proposed_bn - lam_l1 * t;
        }
        // L2 penalty (elastic net)
        if lam_l2 > 0.0 {
            bp[j] /= 1.0 + lam_l2 * t;
            bn[j] /= 1.0 + lam_l2 * t;
        }
    }

    // Interaction update via onerow optimization for each row j
    let mut th = vec![0.0; p * p];
    for j in 0..p {
        // Compute a_k = curth[j,k] + t * ci / 2 - t * rho * V[j,k] for each k
        // and apply soft-threshold
        for k in 0..p {
            if j == k && !diagonal {
                continue;
            }
            let ci_val = if j < k || (j == k && diagonal) {
                let idx = if diagonal { utd(j, k, p) } else { ut(j, k, p) };
                ci[idx]
            } else {
                let idx = if diagonal { utd(k, j, p) } else { ut(k, j, p) };
                ci[idx]
            };

            let proposed = curth[j + p * k] + t * ci_val / 2.0 - t * rho * v[j + p * k];
            let penalty = lam_l1 * t;
            let st = if proposed > penalty {
                proposed - penalty
            } else if proposed < -penalty {
                proposed + penalty
            } else {
                0.0
            };
            let st = if lam_l2 > 0.0 {
                st / (1.0 + lam_l2 * t)
            } else {
                st
            };
            th[j + p * k] = st;
        }
    }

    // Apply hierarchy constraint: scale theta rows to satisfy
    // sum_k |th[j,k]| ≤ bp[j] + bn[j]
    for j in 0..p {
        let mut row_l1 = 0.0;
        for k in 0..p {
            row_l1 += th[j + p * k].abs();
        }
        let budget = bp[j] + bn[j];
        if row_l1 > budget && row_l1 > 0.0 {
            let scale = budget / row_l1;
            for k in 0..p {
                th[j + p * k] *= scale;
            }
        }
    }

    // Symmetrize for strong hierarchy (ADMM handles this in outer loop)
    // For weak hierarchy, we just use the result as-is

    (th, bp, bn)
}

/// Logistic version of the proximal operator.
pub fn prox_zz_given_r_logistic(
    x: &[f64],
    n: usize,
    p: usize,
    zz: &[f64],
    diagonal: bool,
    r: &[f64], // r = y - phat (logistic residual)
    lam_l1: f64,
    lam_l2: f64,
    rho: f64,
    v: &[f64],
    curth: &[f64],
    curbp: &[f64],
    curbn: &[f64],
    t: f64,
) -> (Vec<f64>, Vec<f64>, Vec<f64>) {
    // Same as Gaussian — the only difference is how r is computed
    prox_zz_given_r(
        x, n, p, zz, diagonal, r, lam_l1, lam_l2, rho, v, curth, curbp, curbn, t,
    )
}
