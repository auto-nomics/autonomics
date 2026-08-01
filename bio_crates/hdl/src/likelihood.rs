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

// ===========================================================================
// Precomputed contexts for hot loops (multi-start MLE + profile-CI grid).
//
// These structs precompute per-component terms that are **constant** across
// many likelihood evaluations within one optimisation, eliminating redundant
// arithmetic from the innermost loop.
//
// - [`UnivContext`]: precomputes `lam²`, `lam/Nref`, `lam/N`, `bstar²`.
// - [`GcovContext`]: precomputes `lam11_k`, `lam22_k` (functions of the *fixed*
//   per-trait MLEs — **bit-identical** to the inline computation), plus the
//   `h12`/`int` coefficients.
//
// Coefficient precomputations re-group floating-point operations by ≤1 ULP
// vs the inline functions — well within the cross-validation tolerance (1e-4).
// ===========================================================================

/// Precomputed per-component constants for repeated univariate likelihood
/// evaluation within an optimisation loop.
///
/// Built once for a fixed `(lam, bstar, N, M, Nref, lim)` and reused across
/// all `(h², int)` candidate evaluations.
pub struct UnivContext {
    mf: f64,
    /// `lam[k]²` (precomputed).
    lam_sq: Vec<f64>,
    /// `lam[k] / Nref` (precomputed).
    lam_over_nref: Vec<f64>,
    /// `lam[k] / N` (precomputed).
    lam_over_n: Vec<f64>,
    /// `bstar[k]²` (precomputed).
    bstar_sq: Vec<f64>,
    lim: f64,
}

impl UnivContext {
    /// Build the context from the retained eigen-components.
    pub fn new(lam: &[f64], bstar: &[f64], n: f64, m: usize, nref: f64, lim: f64) -> Self {
        Self {
            mf: m as f64,
            lam_sq: lam.iter().map(|l| l * l).collect(),
            lam_over_nref: lam.iter().map(|l| l / nref).collect(),
            lam_over_n: lam.iter().map(|l| l / n).collect(),
            bstar_sq: bstar.iter().map(|b| b * b).collect(),
            lim,
        }
    }

    /// Univariate log-likelihood at `(h², int)` — equivalent to [`ll_univ`].
    #[inline]
    pub fn ll(&self, h2: f64, int: f64) -> f64 {
        let mut sum_log = 0.0;
        let mut sum_quad = 0.0;
        for k in 0..self.lam_sq.len() {
            let lamh2 = floor_at(
                h2 / self.mf * self.lam_sq[k] - h2 * self.lam_over_nref[k]
                    + int * self.lam_over_n[k],
                self.lim,
            );
            sum_log += lamh2.ln();
            sum_quad += self.bstar_sq[k] / lamh2;
        }
        -0.5 * (sum_log + sum_quad)
    }

    /// Null log-likelihood (`h² = 0`, only `int` is free) — equivalent to
    /// [`ll_univ_null`].
    #[inline]
    pub fn ll_null(&self, int: f64) -> f64 {
        let mut sum_log = 0.0;
        let mut sum_quad = 0.0;
        for k in 0..self.lam_over_n.len() {
            let lamh2 = floor_at(int * self.lam_over_n[k], self.lim);
            sum_log += lamh2.ln();
            sum_quad += self.bstar_sq[k] / lamh2;
        }
        -0.5 * (sum_log + sum_quad)
    }

    /// Evaluate the log-likelihood **and** its gradient w.r.t. `(h², int)` in a
    /// single pass — used by the gradient-aware optimiser to bypass
    /// finite-difference gradient evaluation (3× fewer function calls per
    /// iteration, and no step-size sensitivity).
    ///
    /// When `lamh2_k` is at the `lim` floor, the function is flat w.r.t. further
    /// parameter decreases — the gradient contribution is zeroed to match the
    /// actual (kinked) function surface, matching what finite differences see.
    ///
    /// Returns `(ll, [d_ll/d_h2, d_ll/d_int])`.
    #[inline]
    pub fn ll_grad(&self, h2: f64, int: f64) -> (f64, [f64; 2]) {
        let mut sum_log = 0.0;
        let mut sum_quad = 0.0;
        let mut dll_dh2 = 0.0;
        let mut dll_dint = 0.0;
        for k in 0..self.lam_sq.len() {
            let lamh2_raw = h2 / self.mf * self.lam_sq[k] - h2 * self.lam_over_nref[k]
                + int * self.lam_over_n[k];
            let lamh2 = floor_at(lamh2_raw, self.lim);
            let inv = 1.0 / lamh2;
            let ratio = self.bstar_sq[k] * inv;
            // Gradient is zero when lamh2 is at the floor (kinked surface).
            if lamh2_raw >= self.lim {
                let dlam_dh2 = self.lam_sq[k] / self.mf - self.lam_over_nref[k];
                let dlam_dint = self.lam_over_n[k];
                let common = inv - ratio * inv;
                dll_dh2 += common * dlam_dh2;
                dll_dint += common * dlam_dint;
            }
            sum_log += lamh2.ln();
            sum_quad += ratio;
        }
        (
            -0.5 * (sum_log + sum_quad),
            [-0.5 * dll_dh2, -0.5 * dll_dint],
        )
    }

    /// Null log-likelihood and its gradient w.r.t. `int` (single pass).
    #[inline]
    pub fn ll_null_grad(&self, int: f64) -> (f64, f64) {
        let mut sum_log = 0.0;
        let mut sum_quad = 0.0;
        let mut dll_dint = 0.0;
        for k in 0..self.lam_over_n.len() {
            let lamh2_raw = int * self.lam_over_n[k];
            let lamh2 = floor_at(lamh2_raw, self.lim);
            let inv = 1.0 / lamh2;
            let ratio = self.bstar_sq[k] * inv;
            if lamh2_raw >= self.lim {
                let common = inv - ratio * inv;
                dll_dint += common * self.lam_over_n[k];
            }
            sum_log += lamh2.ln();
            sum_quad += ratio;
        }
        (-0.5 * (sum_log + sum_quad), -0.5 * dll_dint)
    }
}

/// Precomputed per-component constants for repeated conditional genetic-
/// covariance likelihood evaluation.
///
/// `lam11_k` and `lam22_k` depend only on the **fixed** per-trait MLEs
/// `(ĥ², înt)` and are pre-floored once — **bit-identical** to the inline
/// [`ll_gcov`] computation. The `h12`/`int` coefficients and `bstar` values
/// are also precomputed, changing FP grouping by ≤1 ULP.
pub struct GcovContext {
    /// `lam11_k` — pre-floored, constant (trait-1 eigen-variance).
    lam11: Vec<f64>,
    /// `lam22_k` — pre-floored, constant (trait-2 eigen-variance).
    lam22: Vec<f64>,
    /// `bstar1_k` (constant).
    bstar1: Vec<f64>,
    /// `bstar2_k` (constant).
    bstar2: Vec<f64>,
    /// `lam1[k] · lam2[k]` — coefficient of `h12/M`.
    coef_h12: Vec<f64>,
    /// Whether sample overlap is present (`N0 > 0`).
    has_overlap: bool,
    /// `p1·p2·lam1[k]/N0` — coefficient of overlap `int` (empty when no overlap).
    coef_int_overlap: Vec<f64>,
    mf: f64,
    lim: f64,
}

impl GcovContext {
    /// Build the context from the fixed per-trait MLEs and retained eigen-
    /// components.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
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
    ) -> Self {
        let mf = m as f64;
        let p1 = n0 / n1;
        let p2 = n0 / n2;
        let has_overlap = n0 > 0.0;
        let nk = lam1.len();
        let mut lam11 = Vec::with_capacity(nk);
        let mut lam22 = Vec::with_capacity(nk);
        let mut coef_h12 = Vec::with_capacity(nk);
        let mut coef_int_overlap = if has_overlap {
            Vec::with_capacity(nk)
        } else {
            Vec::new()
        };
        for k in 0..nk {
            // lam11/lam22 — same computation as ll_gcov, same result (fixed params).
            lam11.push(floor_at(
                h11[0] / mf * lam1[k] * lam1[k] - h11[0] * lam1[k] / nref + h11[1] * lam1[k] / n1,
                lim,
            ));
            lam22.push(floor_at(
                h22[0] / mf * lam2[k] * lam2[k] - h22[0] * lam2[k] / nref + h22[1] * lam2[k] / n2,
                lim,
            ));
            coef_h12.push(lam1[k] * lam2[k]);
            if has_overlap {
                coef_int_overlap.push(p1 * p2 * lam1[k] / n0);
            }
        }
        Self {
            lam11,
            lam22,
            bstar1: bstar1.to_vec(),
            bstar2: bstar2.to_vec(),
            coef_h12,
            has_overlap,
            coef_int_overlap,
            mf,
            lim,
        }
    }

    /// Conditional genetic-covariance log-likelihood at `(h12, int)` —
    /// equivalent to [`ll_gcov`].
    #[inline]
    pub fn ll(&self, h12: f64, int: f64) -> f64 {
        let mut sum_log = 0.0;
        let mut sum_quad = 0.0;
        for k in 0..self.lam11.len() {
            let lam11_k = self.lam11[k];
            let lam12 = if self.has_overlap {
                h12 / self.mf * self.coef_h12[k] + int * self.coef_int_overlap[k]
            } else {
                h12 / self.mf * self.coef_h12[k]
            };
            let ustar = self.bstar2[k] - lam12 / lam11_k * self.bstar1[k];
            let lam22_1 = floor_at(self.lam22[k] - lam12 * lam12 / lam11_k, self.lim);
            sum_log += lam22_1.ln();
            sum_quad += ustar * ustar / lam22_1;
        }
        -0.5 * (sum_log + sum_quad)
    }

    /// Evaluate the log-likelihood **and** its gradient w.r.t. `(h12, int)` in a
    /// single pass. Used by the gradient-aware optimiser.
    ///
    /// When `lam22_1_k` is at the `lim` floor, the gradient contribution is
    /// zeroed (the function is kinked there — same masking as [`UnivContext`]).
    ///
    /// Returns `(ll, [d_ll/d_h12, d_ll/d_int])`.
    #[inline]
    pub fn ll_grad(&self, h12: f64, int: f64) -> (f64, [f64; 2]) {
        let mut sum_log = 0.0;
        let mut sum_quad = 0.0;
        let mut dll_dh12 = 0.0;
        let mut dll_dint = 0.0;
        for k in 0..self.lam11.len() {
            let lam11_k = self.lam11[k];
            let lam12 = if self.has_overlap {
                h12 / self.mf * self.coef_h12[k] + int * self.coef_int_overlap[k]
            } else {
                h12 / self.mf * self.coef_h12[k]
            };
            let inv_l11 = 1.0 / lam11_k;
            let ustar = self.bstar2[k] - lam12 * inv_l11 * self.bstar1[k];
            let lam22_1_raw = self.lam22[k] - lam12 * lam12 * inv_l11;
            let lam22_1 = floor_at(lam22_1_raw, self.lim);
            let inv_l22_1 = 1.0 / lam22_1;
            let ustar_sq = ustar * ustar;
            // Gradient is zero when lam22_1 is at the floor (kinked surface).
            if lam22_1_raw >= self.lim {
                let dlam12_dh12 = self.coef_h12[k] / self.mf;
                let dlam12_dint = if self.has_overlap {
                    self.coef_int_overlap[k]
                } else {
                    0.0
                };
                let ratio_l12_l11 = lam12 * inv_l11;
                let dlam22_1_dh12 = -2.0 * ratio_l12_l11 * dlam12_dh12;
                let dlam22_1_dint = -2.0 * ratio_l12_l11 * dlam12_dint;
                let dustar_dh12 = -dlam12_dh12 * inv_l11 * self.bstar1[k];
                let dustar_dint = -dlam12_dint * inv_l11 * self.bstar1[k];
                let common_h12 = dlam22_1_dh12 * inv_l22_1 + 2.0 * ustar * dustar_dh12 * inv_l22_1
                    - ustar_sq * dlam22_1_dh12 * inv_l22_1 * inv_l22_1;
                let common_int = dlam22_1_dint * inv_l22_1 + 2.0 * ustar * dustar_dint * inv_l22_1
                    - ustar_sq * dlam22_1_dint * inv_l22_1 * inv_l22_1;
                dll_dh12 += common_h12;
                dll_dint += common_int;
            }
            sum_log += lam22_1.ln();
            sum_quad += ustar_sq * inv_l22_1;
        }
        (
            -0.5 * (sum_log + sum_quad),
            [-0.5 * dll_dh12, -0.5 * dll_dint],
        )
    }
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

    // ---- Context equivalence tests ----

    fn approx_loose(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-10 * b.abs().max(1e-12) + 1e-12
    }

    #[test]
    fn univ_context_matches_inline() {
        let lam = [1.0_f64, 2.0, 0.5, 3.0];
        let bstar = [0.5_f64, -0.3, 0.8, 0.1];
        let lim = (-18.0f64).exp();
        let ctx = UnivContext::new(&lam, &bstar, 1000.0, 4, 335_272.0, lim);
        for &(h2, int) in &[
            (0.1_f64, 1.0_f64),
            (0.0, 1.5),
            (0.5, 0.0),
            (1.0, 20.0),
            (0.3, 5.0),
        ] {
            let a = ll_univ(h2, int, 1000.0, 4, 335_272.0, &lam, &bstar, lim);
            let b = ctx.ll(h2, int);
            assert!(
                approx_loose(a, b),
                "h2={h2} int={int}: inline={a:.12e} ctx={b:.12e}"
            );
        }
        // null
        for &int in &[0.5_f64, 1.0, 10.0] {
            let a = ll_univ_null(int, 1000.0, 4, 335_272.0, &lam, &bstar, lim);
            let b = ctx.ll_null(int);
            assert!(
                approx_loose(a, b),
                "int={int}: inline={a:.12e} ctx={b:.12e}"
            );
        }
    }

    #[test]
    fn gcov_context_matches_inline() {
        let h11 = [0.1_f64, 1.0];
        let h22 = [0.2, 1.1];
        let lam = [1.0_f64, 2.0, 0.5, 1.5];
        let b1 = [0.4_f64, -0.2, 0.3, 0.6];
        let b2 = [0.3_f64, 0.1, -0.5, 0.2];
        let lim = (-18.0f64).exp();
        // no overlap
        let ctx = GcovContext::new(
            &h11, &h22, 4, 1000.0, 2000.0, 0.0, 335_272.0, &lam, &lam, &b1, &b2, lim,
        );
        for &(h12, int) in &[(0.0_f64, 0.7_f64), (0.05, 1.0), (-0.03, 0.5), (0.1, 5.0)] {
            let a = ll_gcov(
                h12, int, &h11, &h22, 4, 1000.0, 2000.0, 0.0, 335_272.0, &lam, &lam, &b1, &b2, lim,
            );
            let b = ctx.ll(h12, int);
            assert!(
                approx_loose(a, b),
                "h12={h12} int={int}: inline={a:.12e} ctx={b:.12e}"
            );
        }
        // with overlap
        let ctx_ov = GcovContext::new(
            &h11, &h22, 4, 1000.0, 2000.0, 500.0, 335_272.0, &lam, &lam, &b1, &b2, lim,
        );
        for &(h12, int) in &[(0.05_f64, 0.7_f64), (-0.02, 1.5), (0.1, 0.0)] {
            let a = ll_gcov(
                h12, int, &h11, &h22, 4, 1000.0, 2000.0, 500.0, 335_272.0, &lam, &lam, &b1, &b2,
                lim,
            );
            let b = ctx_ov.ll(h12, int);
            assert!(
                approx_loose(a, b),
                "ov h12={h12} int={int}: inline={a:.12e} ctx={b:.12e}"
            );
        }
    }

    // ---- Analytical gradient validation against finite differences ----

    fn fd_grad(f: impl Fn(f64, f64) -> f64, h2: f64, int: f64, h: f64) -> [f64; 2] {
        [
            (f(h2 + h, int) - f(h2 - h, int)) / (2.0 * h),
            (f(h2, int + h) - f(h2, int - h)) / (2.0 * h),
        ]
    }

    #[test]
    fn univ_grad_matches_fd() {
        let lam = [1.0_f64, 2.0, 0.5, 3.0, 1.5];
        let bstar = [0.5_f64, -0.3, 0.8, 0.1, -0.4];
        let lim = (-18.0f64).exp();
        let ctx = UnivContext::new(&lam, &bstar, 1000.0, 5, 335_272.0, lim);
        for &(h2, int) in &[(0.1_f64, 1.0_f64), (0.3, 5.0), (0.5, 2.0)] {
            let (_, grad) = ctx.ll_grad(h2, int);
            let fd = fd_grad(|a, b| ctx.ll(a, b), h2, int, 1e-6);
            let err0 = (grad[0] - fd[0]).abs();
            let err1 = (grad[1] - fd[1]).abs();
            assert!(
                err0 < 1e-5,
                "univ grad h2: analytical={:.6e} fd={:.6e} err={:.2e}",
                grad[0],
                fd[0],
                err0
            );
            assert!(
                err1 < 1e-5,
                "univ grad int: analytical={:.6e} fd={:.6e} err={:.2e}",
                grad[1],
                fd[1],
                err1
            );
        }
    }

    #[test]
    fn gcov_grad_matches_fd() {
        let h11 = [0.1_f64, 1.0];
        let h22 = [0.2, 1.1];
        let lam = [1.0_f64, 2.0, 0.5, 1.5];
        let b1 = [0.4_f64, -0.2, 0.3, 0.6];
        let b2 = [0.3_f64, 0.1, -0.5, 0.2];
        let lim = (-18.0f64).exp();
        // no overlap
        let ctx = GcovContext::new(
            &h11, &h22, 4, 1000.0, 2000.0, 0.0, 335_272.0, &lam, &lam, &b1, &b2, lim,
        );
        for &(h12, int) in &[(0.05_f64, 0.7_f64), (-0.02, 1.0), (0.08, 2.0)] {
            let (_, grad) = ctx.ll_grad(h12, int);
            let fd = fd_grad(|a, b| ctx.ll(a, b), h12, int, 1e-6);
            let err0 = (grad[0] - fd[0]).abs();
            let err1 = (grad[1] - fd[1]).abs();
            assert!(
                err0 < 1e-4,
                "gcov grad h12: analytical={:.6e} fd={:.6e} err={:.2e}",
                grad[0],
                fd[0],
                err0
            );
            assert!(
                err1 < 1e-4,
                "gcov grad int: analytical={:.6e} fd={:.6e} err={:.2e}",
                grad[1],
                fd[1],
                err1
            );
        }
        // with overlap
        let ctx_ov = GcovContext::new(
            &h11, &h22, 4, 1000.0, 2000.0, 500.0, 335_272.0, &lam, &lam, &b1, &b2, lim,
        );
        for &(h12, int) in &[(0.05_f64, 0.7_f64), (0.03, 1.5)] {
            let (_, grad) = ctx_ov.ll_grad(h12, int);
            let fd = fd_grad(|a, b| ctx_ov.ll(a, b), h12, int, 1e-6);
            assert!(
                (grad[0] - fd[0]).abs() < 1e-4,
                "gcov ov grad h12: ana={:.6e} fd={:.6e}",
                grad[0],
                fd[0]
            );
            assert!(
                (grad[1] - fd[1]).abs() < 1e-4,
                "gcov ov grad int: ana={:.6e} fd={:.6e}",
                grad[1],
                fd[1]
            );
        }
    }
}
