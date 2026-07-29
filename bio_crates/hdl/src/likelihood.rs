//! The eigen-space HDL log-likelihoods — 1:1 port of the four functions inlined
//! in `HDL/R/HDL.L.R` (lines 73-137) plus `HDL/R/llfun.R` and
//! `HDL/R/llfun.gcov.part.2.R`.
//!
//! All functions return the **log-likelihood** `ll` (the `-1/2·(Σlog + Σ·)` form),
//! to be **maximised** — matching R's `optim(..., fnscale = -1)`.
//!
//! Notation (per retained eigen-component `i`, after the variance eigen-cut):
//! - `lam[i]`  — eigenvalue of the LD correlation matrix R,
//! - `bstar[i] = Vᵀ·bhat` — the eigen-transformed effect estimate (`bhat = Z/√N`).
//!
//! ## Univariate h²
//! `llfun`: `lamh2 = h²/M·lam² − h²·lam/Nref + int·lam/N`, floored at `lim`;
//! `ll = −½(Σ log(lamh2) + Σ bstar²/lamh2)`.
//! `llfun0`: the null form with `h² = 0` → `lamh2 = int·lam/N`.
//!
//! ## Conditional genetic covariance (h₁₂ | ĥ²₁, ĥ²₂)
//! `llfun.gcov.part.2`: builds `lam11/lam22` from the fixed `(ĥ², înt)` of each
//! trait, `lam12` from `h₁₂` (plus a sample-overlap term when `N0 > 0`), then
//! the conditional (z₂ | z₁) residual `ustar = bstar2 − (lam12/lam11)·bstar1`
//! with `lam22.1 = lam22 − lam12²/lam11`, floored at `lim`.
//! `llfun0.gcov.part.2`: null form with `h₁₂ = 0`.
//!
//! `lim = exp(-18)` floors every eigen-term away from zero (R default).

/// Floor `x` at `lim` (R `ifelse(x < lim, lim, x)`).
#[inline]
fn floor_at(x: f64, lim: f64) -> f64 {
    if x < lim { lim } else { x }
}

/// Univariate HDL log-likelihood of one trait.
///
/// `param = [h2, int]`, `lam`/`bstar` are the retained eigen-components.
/// Faithful port of `HDL.L.R::llfun` (lines 73-79).
#[allow(clippy::too_many_arguments)]
pub fn ll_univ(
    h2: f64,
    int: f64,
    n: f64,
    m: usize,
    nref: f64,
    lam: &[f64],
    bstar: &[f64],
    lim: f64,
) -> f64 {
    let mf = m as f64;
    let mut sum_log = 0.0;
    let mut sum_quad = 0.0;
    for k in 0..lam.len() {
        let lamh2 = floor_at(
            h2 / mf * lam[k] * lam[k] - h2 * lam[k] / nref + int * lam[k] / n,
            lim,
        );
        sum_log += lamh2.ln();
        sum_quad += bstar[k] * bstar[k] / lamh2;
    }
    -0.5 * (sum_log + sum_quad)
}

/// Univariate HDL log-likelihood under the null `h² = 0` (intercept only).
///
/// Faithful port of `HDL.L.R::llfun0` (lines 82-87). Only `int` is free;
/// `m`/`nref` are kept in the signature to mirror R's `llfun0(int, N, M, Nref, …)`.
#[allow(unused_variables)]
pub fn ll_univ_null(
    int: f64,
    n: f64,
    m: usize,
    nref: f64,
    lam: &[f64],
    bstar: &[f64],
    lim: f64,
) -> f64 {
    // h2 = 0 ⇒ lamh2 = int * lam / n
    let mut sum_log = 0.0;
    let mut sum_quad = 0.0;
    for k in 0..lam.len() {
        let lamh2 = floor_at(int * lam[k] / n, lim);
        sum_log += lamh2.ln();
        sum_quad += bstar[k] * bstar[k] / lamh2;
    }
    -0.5 * (sum_log + sum_quad)
}

/// Conditional genetic-covariance log-likelihood `ℒ(h₁₂ | ĥ²₁, ĥ²₂)`.
///
/// `h11 = [h²₁, int₁]`, `h22 = [h²₂, int₂]` are the per-trait MLEs (fixed).
/// `param = [h12, int]`. `lam1`/`lam2` are the retained eigen-components (in the
/// no-overlap reference they coincide; kept separate to mirror R exactly).
/// Faithful port of `HDL.L.R::llfun.gcov.part.2` (lines 92-114) /
/// `HDL/R/llfun.gcov.part.2.R`.
#[allow(clippy::too_many_arguments)]
pub fn ll_gcov(
    h12: f64,
    int: f64,
    h11: &[f64; 2],
    h22: &[f64; 2],
    m: usize,
    n1: f64,
    n2: f64,
    n0: f64,
    nref: f64,
    lam1: &[f64],
    lam2: &[f64],
    bstar1: &[f64],
    bstar2: &[f64],
    lim: f64,
) -> f64 {
    let mf = m as f64;
    let p1 = n0 / n1;
    let p2 = n0 / n2;

    let mut sum_log = 0.0;
    let mut sum_quad = 0.0;
    for k in 0..lam1.len() {
        // lam11 uses trait-1 (h², int); lam22 uses trait-2.
        let lam11 = floor_at(
            h11[0] / mf * lam1[k] * lam1[k] - h11[0] * lam1[k] / nref + h11[1] * lam1[k] / n1,
            lim,
        );
        let lam22 = floor_at(
            h22[0] / mf * lam2[k] * lam2[k] - h22[0] * lam2[k] / nref + h22[1] * lam2[k] / n2,
            lim,
        );
        // lam12: genetic covariance + (sample-overlap intercept term when N0 > 0).
        let lam12 = if n0 > 0.0 {
            h12 / mf * lam1[k] * lam2[k] + p1 * p2 * int * lam1[k] / n0
        } else {
            h12 / mf * lam1[k] * lam2[k]
        };
        // Conditional (z₂ | z₁): residual + conditional variance.
        let ustar = bstar2[k] - lam12 / lam11 * bstar1[k];
        let lam22_1 = floor_at(lam22 - lam12 * lam12 / lam11, lim);
        sum_log += lam22_1.ln();
        sum_quad += ustar * ustar / lam22_1;
    }
    -0.5 * (sum_log + sum_quad)
}

/// Conditional genetic-covariance log-likelihood under the null `h₁₂ = 0`.
///
/// Only `int` is free. Faithful port of `HDL.L.R::llfun0.gcov.part.2`
/// (lines 117-138).
#[allow(clippy::too_many_arguments)]
pub fn ll_gcov_null(
    int: f64,
    h11: &[f64; 2],
    h22: &[f64; 2],
    m: usize,
    n1: f64,
    n2: f64,
    n0: f64,
    nref: f64,
    lam1: &[f64],
    lam2: &[f64],
    bstar1: &[f64],
    bstar2: &[f64],
    lim: f64,
) -> f64 {
    // h12 = 0; reuse ll_gcov with h12 = 0.
    ll_gcov(
        0.0, int, h11, h22, m, n1, n2, n0, nref, lam1, lam2, bstar1, bstar2, lim,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9 * b.abs().max(1e-12) + 1e-12
    }

    #[test]
    fn univ_ll_handcheck() {
        // lam = [1, 2], bstar = [0.5, -0.3], h2=0.1, int=1.0, N=1000, M=2, Nref=335272
        let lam = [1.0_f64, 2.0];
        let bstar = [0.5_f64, -0.3];
        let lim = (-18.0f64).exp();
        let ll = ll_univ(0.1, 1.0, 1000.0, 2, 335272.0, &lam, &bstar, lim);
        // hand compute lamh2[k] = h2/M*lam^2 - h2*lam/Nref + int*lam/N
        // k0: 0.1/2*1 - 0.1*1/335272 + 1*1/1000 = 0.05 - 2.98e-7 + 0.001 = 0.051000(ish)
        let l0: f64 = 0.1 / 2.0 * 1.0 - 0.1 * 1.0 / 335272.0 + 1.0 * 1.0 / 1000.0;
        let l1: f64 = 0.1 / 2.0 * 4.0 - 0.1 * 2.0 / 335272.0 + 1.0 * 2.0 / 1000.0;
        let expect = -0.5 * ((l0.ln() + l1.ln()) + (0.25 / l0 + 0.09 / l1));
        assert!(approx(ll, expect), "ll={ll} expect={expect}");
    }

    #[test]
    fn null_h2_is_intercept_only() {
        // ll_univ with h2=0 must equal ll_univ_null for the same int.
        let lam = [1.0_f64, 2.0, 0.5];
        let bstar = [0.5_f64, -0.3, 0.8];
        let lim = (-18.0f64).exp();
        let a = ll_univ(0.0, 1.5, 1000.0, 3, 335272.0, &lam, &bstar, lim);
        let b = ll_univ_null(1.5, 1000.0, 3, 335272.0, &lam, &bstar, lim);
        assert!(approx(a, b), "a={a} b={b}");
    }

    #[test]
    fn gcov_null_is_h12_zero() {
        let h11 = [0.1_f64, 1.0];
        let h22 = [0.2, 1.1];
        let lam = [1.0_f64, 2.0];
        let b1 = [0.4_f64, -0.2];
        let b2 = [0.3_f64, 0.1];
        let lim = (-18.0f64).exp();
        let a = ll_gcov(
            0.0, 0.7, &h11, &h22, 2, 1000.0, 2000.0, 0.0, 335272.0, &lam, &lam, &b1, &b2, lim,
        );
        let b = ll_gcov_null(
            0.7, &h11, &h22, 2, 1000.0, 2000.0, 0.0, 335272.0, &lam, &lam, &b1, &b2, lim,
        );
        assert!(approx(a, b), "a={a} b={b}");
    }

    #[test]
    fn lim_floor_prevents_neg_lamh2() {
        // Extreme params that would make lamh2 negative must not panic (ln of lim).
        let lam = [1.0_f64];
        let bstar = [1.0_f64];
        let lim = (-18.0f64).exp();
        let ll = ll_univ(-5.0, -5.0, 1000.0, 1, 335272.0, &lam, &bstar, lim);
        assert!(ll.is_finite(), "ll={ll}");
    }
}
