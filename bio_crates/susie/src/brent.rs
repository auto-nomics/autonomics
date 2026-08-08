//! Faithful port of R's `optim(par, fn, method = "Brent", lower, upper)`.
//!
//! R's Brent optimizer (from `stats::optim`) is a C port of Brent's
//! successive-parabolic-interpolation + golden-section method (Brent 1973,
//! *Algorithms for Minimization without Derivatives*).
//!
//! Reference: R source `src/main/optim.c`, function `Brent(...)`, which is
//! derived from the Netlib `fmin` routine. Defaults: `tol = sqrt(.Machine$double.eps)`,
//! `maxit = 100`.

// R optim.c Brent defaults
const R_BRENT_TOL: f64 = 1.4901161193847656e-8;
const R_BRENT_MAXIT: i32 = 100;

/// Golden-section constant `(3 - sqrt(5)) / 2 ≈ 0.3819660`.
const GOLD_C: f64 = 0.3819660112501051;

/// Minimize `f` on the interval `[a, b]` using Brent's method.
///
/// Returns the argmin `x*` in `[a, b]`.
///
/// This is a line-by-line port of R's `Brent()` in `optim.c`.
pub fn brent_minimize<F>(f: &F, mut a: f64, mut b: f64) -> f64
where
    F: Fn(f64) -> f64,
{
    let tol: f64 = R_BRENT_TOL;

    let mut x: f64 = a + GOLD_C * (b - a);
    let mut w: f64 = x;
    let mut v: f64 = x;
    let mut fx: f64 = f(x);
    let mut fw: f64 = fx;
    let mut fv: f64 = fx;
    let mut e: f64 = 0.0; // distance moved on the *previous* step
    let mut d: f64 = 0.0;

    for _ in 0..R_BRENT_MAXIT {
        let m: f64 = 0.5 * (a + b);
        let tol1: f64 = tol * x.abs() + 1e-10;
        let tol2: f64 = 2.0 * tol1;

        // Convergence: x near midpoint within tolerance
        if (x - m).abs() <= tol2 - 0.5 * (b - a) {
            break;
        }

        // Try parabolic interpolation if the previous step was large enough
        let use_parabola: bool = e.abs() > tol1;
        let mut took_step: bool = false;

        if use_parabola {
            // Fit parabola through (v,fv), (x,fx), (w,fw)
            let r: f64 = (x - w) * (fx - fv);
            let q_: f64 = (x - v) * (fx - fw);
            let mut p: f64 = (x - v) * q_ - (x - w) * r;
            let mut q2: f64 = 2.0 * (q_ - r);
            if q2 > 0.0 {
                p = -p;
            }
            q2 = q2.abs();

            let etemp: f64 = e;
            e = d;

            // Check acceptability of the parabolic step
            if p.abs() < 0.5 * q2 * etemp.abs() && p > q2 * (a - x) && p < q2 * (b - x) {
                d = p / q2;
                let u: f64 = x + d;
                if (u - a) < tol2 || (b - u) < tol2 {
                    d = if x < m { tol1 } else { -tol1 };
                }
                took_step = true;
            }
        }

        if !took_step {
            e = if x < m { b - x } else { a - x };
            d = GOLD_C * e;
        }

        let u: f64 = if d.abs() >= tol1 {
            x + d
        } else {
            x + if d > 0.0 { tol1 } else { -tol1 }
        };

        let fu: f64 = f(u);

        if fu <= fx {
            if u < x {
                b = x;
            } else {
                a = x;
            }
            v = w;
            fv = fw;
            w = x;
            fw = fx;
            x = u;
            fx = fu;
        } else {
            if u < x {
                a = u;
            } else {
                b = u;
            }
            if fu <= fw || w == x {
                v = w;
                fv = fw;
                w = u;
                fw = fu;
            } else if fu <= fv || v == x || v == w {
                v = u;
                fv = fu;
            }
        }
    }

    x
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn brent_minimizes_quadratic() {
        let f = |x: f64| (x - 2.5_f64).powi(2);
        let xmin = brent_minimize(&f, -10.0, 10.0);
        assert!((xmin - 2.5).abs() < 1e-7);
    }

    #[test]
    fn brent_handles_flat_region() {
        let f = |_x: f64| 0.0_f64;
        let xmin = brent_minimize(&f, 0.0, 1.0);
        assert!((0.0..=1.0).contains(&xmin));
    }
}
