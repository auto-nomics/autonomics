//! Special functions needed to port the `scipy.stats` quantities LDSC uses
//! (`chi2.sf`, `chi2.isf`, `norm.isf`, `norm.pdf`) — implemented dependency-free
//! so the crate stays pure-Rust.
//!
//! - [`erfc`] — complementary error function (Numerical Recipes `erfcc`,
//!   relative error < ~1.2e-7). Used for `chi2.sf(z², 1)`.
//! - [`norm_ppf`] — inverse standard-normal CDF (Acklam's rational
//!   approximation, absolute error < ~1.15e-9). Used for `norm.isf` /
//!   `chi2.isf`.

use std::f64::consts::{FRAC_1_SQRT_2, PI, SQRT_2};

/// Complementary error function `erfc(x)`, relative error < ~1.2e-7
/// (Numerical Recipes `erfcc`).
pub fn erfc(x: f64) -> f64 {
    let z = x.abs();
    let t = 1.0 / (1.0 + 0.5 * z);
    let ans = t
        * (-z * z - 1.26551223
            + t * (1.00002368
                + t * (0.37409196
                    + t * (0.09678418
                        + t * (-0.18628806
                            + t * (0.27886807
                                + t * (-1.13520398
                                    + t * (1.48851587 + t * (-0.82215223 + t * 0.17087277)))))))))
            .exp();
    if x >= 0.0 { ans } else { 2.0 - ans }
}

/// Inverse standard-normal CDF `Φ⁻¹(p)` (Acklam). Absolute error < ~1.15e-9 over
/// the central region; tails refined by one Halley step.
pub fn norm_ppf(p: f64) -> f64 {
    // Acklam constants
    let a = [
        -3.969683028665376e+01,
        2.209460984245205e+02,
        -2.759285104469687e+02,
        1.383_577_518_672_69e2,
        -3.066479806614716e+01,
        2.506628277459239e+00,
    ];
    let b = [
        -5.447609879822406e+01,
        1.615858368580409e+02,
        -1.556989798598866e+02,
        6.680131188771972e+01,
        -1.328068155288572e+01,
    ];
    let c = [
        -7.784894002430293e-03,
        -3.223964580411365e-01,
        -2.400758277161838e+00,
        -2.549732539343734e+00,
        4.374664141464968e+00,
        2.938163982698783e+00,
    ];
    let d = [
        7.784695709041462e-03,
        3.224671290700398e-01,
        2.445134137142996e+00,
        3.754408661907416e+00,
    ];
    let plow = 0.02425;
    let phigh = 1.0 - plow;

    if p <= 0.0 {
        return f64::NEG_INFINITY;
    }
    if p >= 1.0 {
        return f64::INFINITY;
    }

    let mut x;
    if p < plow {
        let q = (-2.0 * p.ln()).sqrt();
        x = (((((c[0] * q + c[1]) * q + c[2]) * q + c[3]) * q + c[4]) * q + c[5])
            / ((((d[0] * q + d[1]) * q + d[2]) * q + d[3]) * q + 1.0);
    } else if p <= phigh {
        let q = p - 0.5;
        let r = q * q;
        x = (((((a[0] * r + a[1]) * r + a[2]) * r + a[3]) * r + a[4]) * r + a[5]) * q
            / (((((b[0] * r + b[1]) * r + b[2]) * r + b[3]) * r + b[4]) * r + 1.0);
    } else {
        let q = (-2.0 * (1.0 - p).ln()).sqrt();
        x = -(((((c[0] * q + c[1]) * q + c[2]) * q + c[3]) * q + c[4]) * q + c[5])
            / ((((d[0] * q + d[1]) * q + d[2]) * q + d[3]) * q + 1.0);
    }

    // One Halley refinement step for full machine precision.
    let e = 0.5 * erfc(-x / SQRT_2) - p;
    let u = e * (2.0 * PI).sqrt() * (x * x / 2.0).exp();
    x = x - u / (1.0 + x * u / 2.0);
    x
}

/// Standard-normal survival `P(Z > z) = 1 - Φ(z)`.
pub fn norm_sf(z: f64) -> f64 {
    0.5 * erfc(z * FRAC_1_SQRT_2)
}

/// Standard-normal inverse survival `norm.isf(p)` = `Φ⁻¹(1-p)`.
pub fn norm_isf(p: f64) -> f64 {
    norm_ppf(1.0 - p)
}

/// Standard-normal density.
pub fn norm_pdf(z: f64) -> f64 {
    (-0.5 * z * z).exp() / (2.0 * PI).sqrt()
}

/// `chi2.sf(x, 1)` — upper tail of a χ² with 1 dof = `P(|Z| > √x) = erfc(√(x/2))`.
pub fn chi2_sf_1(x: f64) -> f64 {
    if x <= 0.0 {
        return 1.0;
    }
    erfc((x * 0.5).sqrt())
}

/// `chi2.isf(p, 1)` — inverse: `norm.isf(p/2)²`.
pub fn chi2_isf_1(p: f64) -> f64 {
    let z = norm_isf(p * 0.5);
    z * z
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: f64, b: f64, tol: f64) -> bool {
        (a - b).abs() <= tol * b.abs().max(1.0)
    }

    #[test]
    fn erfc_known_values() {
        // NR erfcc: relative error < ~1.2e-7 — unit-test tolerance matches that.
        assert!(approx(erfc(0.0), 1.0, 1e-6));
        assert!(approx(erfc(1.0), 0.15729920705028513, 1e-6));
        assert!(approx(erfc(0.5), 0.4795001221869535, 1e-6));
        assert!(approx(erfc(-1.0), 1.842700792949715, 1e-6));
    }

    #[test]
    fn norm_ppf_known() {
        assert!(approx(norm_ppf(0.975), 1.959963985, 1e-6));
        assert!(approx(norm_ppf(0.99), 2.326347874, 1e-6));
        assert!(approx(norm_ppf(0.5), 0.0, 1e-6));
    }

    #[test]
    fn chi2_sf_p_z_norm() {
        // p_z_norm(10,1): P = chi2.sf(100,1) ≈ 1.523971e-23
        let p = chi2_sf_1(100.0);
        assert!((p * 1e23 - 1.523971).abs() < 1e-3, "p*1e23 = {}", p * 1e23);
    }

    #[test]
    fn chi2_isf_p_to_z() {
        // p_to_z(0.1) = sqrt(chi2.isf(0.1,1)) = 1.644854
        let z = chi2_isf_1(0.1).sqrt();
        assert!((z - 1.644854).abs() < 1e-4, "z = {z}");
    }
}
