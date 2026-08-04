//! Core mathematical utility functions ported from `R/utils.R` and
//! the internal helpers in `R/EValue.R`.

use crate::error::{EvalueError, Result};

/// Compute the E-value for a single risk-ratio value.
///
/// `threshold(x, true_val)` gives the minimum bias factor on the RR scale
/// needed to shift the observed RR `x` to `true_val`.
///
/// Direct port of `threshold()` in `EValue.R:437`.
pub fn threshold(x: f64, true_val: f64) -> Option<f64> {
    if x.is_nan() {
        return None;
    }
    if x < 0.0 {
        // R issues a warning; we still compute
    }

    let (x, true_val) = if x <= 1.0 {
        (1.0 / x, 1.0 / true_val)
    } else {
        (x, true_val)
    };

    if true_val <= x {
        // causal effect is toward null
        Some((x + (x * (x - true_val)).sqrt()) / true_val)
    } else {
        // causal effect is away from null
        let rat = true_val / x;
        Some(rat + (rat * (rat - 1.0)).sqrt())
    }
}

/// Bias factor from two risk-ratio components.
///
/// `bf_func(rr1, rr2) = (rr1 * rr2) / (rr1 + rr2 - 1)`
///
/// Direct port of `bf_func()` in `utils.R:26`.
pub fn bf_func(rr1: f64, rr2: f64) -> f64 {
    (rr1 * rr2) / (rr1 + rr2 - 1.0)
}

/// Degree polynomial used for multi-bias root finding.
///
/// `deg_func(x, y, n, d) = x^n / (2x - 1)^d - y`
///
/// Direct port of `deg_func()` in `utils.R:12`.
pub fn deg_func(x: f64, y: f64, n: i32, d: i32) -> f64 {
    x.powi(n) / (2.0 * x - 1.0).powi(d) - y
}

/// Transformation from bias factor to confounding strength scale.
///
/// `g(x) = x + sqrt(x^2 - x)` for `x >= 1`, else a tiny value.
///
/// Direct port of `g()` in `utils.R:35`. Vectorized in R, scalar here.
pub fn g(x: f64) -> f64 {
    if x.is_nan() {
        return f64::NAN;
    }
    if x < 1.0 {
        return x / 1e10;
    }
    x + (x * x - x).sqrt()
}

/// Inverse of [`g`]: given a confounding strength, return the bias factor.
/// `s = g(x) = x + sqrt(x^2 - x)`, so `(s - x)^2 = x^2 - x`,
/// `s^2 - 2sx + x^2 = x^2 - x`, `s^2 = x(2s - 1)`, `x = s^2 / (2s - 1)`.
pub fn g_inv(s: f64) -> f64 {
    if s.is_nan() || s < 1.0 {
        return 1.0;
    }
    s * s / (2.0 * s - 1.0)
}

/// Brent's method for root finding in a bracketed interval.
///
/// Port of R's `stats::uniroot`. Finds `x` in `[a, b]` where `f(x) = 0`.
/// Requires `f(a)` and `f(b)` have opposite signs (or one is zero).
/// Uses Brent-Dekker algorithm: inverse quadratic interpolation when
/// possible, falling back to bisection.
pub fn brent_root<F>(f: F, a: f64, b: f64, tol: f64, max_iter: usize) -> Result<f64>
where
    F: Fn(f64) -> f64,
{
    let mut a = a;
    let mut b = b;
    let mut fa = f(a);
    let mut fb = f(b);

    if fa == 0.0 {
        return Ok(a);
    }
    if fb == 0.0 {
        return Ok(b);
    }
    if fa * fb > 0.0 {
        return Err(EvalueError::Compute(format!(
            "brent_root: f({a})={fa:.6e} and f({b})={fb:.6e} have the same sign; root not bracketed"
        )));
    }

    // Ensure |fa| >= |fb|, so b is the best guess
    if fa.abs() < fb.abs() {
        std::mem::swap(&mut a, &mut b);
        std::mem::swap(&mut fa, &mut fb);
    }

    let mut c = a;
    let mut fc = fa;
    let mut mflag = true;
    let mut d = c; // only used when mflag is false

    for _ in 0..max_iter {
        // Convergence check
        if (b - a).abs() < tol || fb.abs() < tol {
            return Ok(b);
        }

        let s = if fa != fc && fb != fc {
            // Inverse quadratic interpolation
            (a * fb * fc) / ((fa - fb) * (fa - fc))
                + (b * fa * fc) / ((fb - fa) * (fb - fc))
                + (c * fa * fb) / ((fc - fa) * (fc - fb))
        } else {
            // Secant method
            b - fb * (b - a) / (fb - fa)
        };

        // Check if s is acceptable; if not, use bisection
        let cond1 = !(s >= (3.0 * a + b) / 4.0 && s <= b) && !(s <= (3.0 * a + b) / 4.0 && s >= b);
        let cond2 = mflag && (s - b).abs() >= (b - c).abs() / 2.0;
        let cond3 = !mflag && (s - b).abs() >= (c - d).abs() / 2.0;
        let cond4 = mflag && (b - c).abs() < tol;
        let cond5 = !mflag && (c - d).abs() < tol;

        let s = if cond1 || cond2 || cond3 || cond4 || cond5 {
            // Bisection
            (a + b) / 2.0
        } else {
            s
        };

        let fs = f(s);

        d = c;
        c = b;
        fc = fb;

        if fa * fs < 0.0 {
            b = s;
            fb = fs;
        } else {
            a = s;
            fa = fs;
        }

        // Ensure |fa| >= |fb|
        if fa.abs() < fb.abs() {
            std::mem::swap(&mut a, &mut b);
            std::mem::swap(&mut fa, &mut fb);
        }

        mflag = false;
    }

    Ok(b)
}

/// Golden-section search for minimization in a bracketed interval.
///
/// Used as a simpler alternative to `optimize()` in R for the effect-mod
/// E-value grid search.
pub fn minimize_golden<F>(f: F, a: f64, b: f64, tol: f64, max_iter: usize) -> (f64, f64)
where
    F: Fn(f64) -> f64,
{
    let gr = (5f64.sqrt() - 1.0) / 2.0; // golden ratio ≈ 0.618
    let mut a = a;
    let mut b = b;
    let mut c = b - gr * (b - a);
    let mut d = a + gr * (b - a);
    let mut fc = f(c);
    let mut fd = f(d);

    for _ in 0..max_iter {
        if fc < fd {
            b = d;
            d = c;
            fd = fc;
            c = b - gr * (b - a);
            fc = f(c);
        } else {
            a = c;
            c = d;
            fc = fd;
            d = a + gr * (b - a);
            fd = f(d);
        }
        if (b - a).abs() < tol {
            break;
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
    fn test_threshold_basic() {
        // From VanderWeele & Ding (2017): RR = 0.80
        // E-value for point = 1.809
        let e = threshold(0.80, 1.0).unwrap();
        assert!((e - 1.809).abs() < 0.001, "got {e}");
    }

    #[test]
    fn test_threshold_symmetry() {
        // E-value is symmetric: RR and 1/RR give same E-value
        let e1 = threshold(0.80, 1.0).unwrap();
        let e2 = threshold(1.0 / 0.80, 1.0).unwrap();
        assert!((e1 - e2).abs() < 1e-10);
    }

    #[test]
    fn test_threshold_smoking() {
        // Hammond & Horn: RR ≈ 10.73 → E-value ≈ 20.95
        let e = threshold(10.73, 1.0).unwrap();
        assert!((e - 20.94777).abs() < 0.001, "got {e}");
    }

    #[test]
    fn test_threshold_non_null() {
        // Non-null: threshold(RR=2, true=1.5)
        let e = threshold(2.0, 1.5).unwrap();
        // rat = 2/1.5 = 1.333..., e = rat + sqrt(rat*(rat-1))
        let rat: f64 = 2.0 / 1.5;
        let expected = rat + (rat * (rat - 1.0_f64)).sqrt();
        assert!((e - expected).abs() < 1e-10);
    }

    #[test]
    fn test_threshold_true_greater() {
        // true > x case: x=0.5, true=1.0 → after inversion x=2, true=1
        // Actually: x=0.5 <= 1, so x becomes 2, true becomes 1
        // true(1) <= x(2), so standard case
        let e = threshold(0.5, 1.0).unwrap();
        let expected = (2.0 + (2.0_f64 * 1.0_f64).sqrt()) / 1.0;
        assert!((e - expected).abs() < 1e-10);
    }

    #[test]
    fn test_bf_func() {
        let v = bf_func(2.0, 2.0);
        // (4)/(3) = 1.333...
        assert!((v - 4.0 / 3.0).abs() < 1e-10);
    }

    #[test]
    fn test_g_function() {
        // g(1) = 1 + sqrt(0) = 1
        assert!((g(1.0) - 1.0).abs() < 1e-10);
        // g(2) = 2 + sqrt(2) = 3.414...
        assert!((g(2.0) - (2.0 + 2f64.sqrt())).abs() < 1e-10);
    }

    #[test]
    fn test_g_inv_inverse() {
        for &x in &[1.0, 1.5, 2.0, 3.0, 5.0, 10.0] {
            let s = g(x);
            let x_back = g_inv(s);
            assert!((x - x_back).abs() < 1e-8, "g_inv(g({x})) = {x_back}, expected {x}");
        }
    }

    #[test]
    fn test_brent_root() {
        // Solve x^2 - 4 = 0 in [1, 3] → 2
        let root = brent_root(|x| x * x - 4.0, 1.0, 3.0, 1e-10, 100).unwrap();
        assert!((root - 2.0).abs() < 1e-8, "got {root}");
    }

    #[test]
    fn test_brent_root_deg_func() {
        // Solve deg_func(x, 2, 2, 1) = x^2/(2x-1) - 2 = 0
        // Root at 2+sqrt(2) ≈ 3.414
        let root = brent_root(|x| deg_func(x, 2.0, 2, 1), 1.0 + 1e-9, 4.0, 1e-10, 100).unwrap();
        assert!(deg_func(root, 2.0, 2, 1).abs() < 1e-8, "root={root}, f(root)={}", deg_func(root, 2.0, 2, 1));
    }
}
