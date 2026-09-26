//! Tail probabilities of weighted sums of chi-squares — Rust port of the
//! `survey` package's `pchisqsum`/`pFsum` (files `R/chisqsum.R`, `R/pFsum.R`).
//!
//! The survey-LRT ([`crate::model::svy_lrt`]) compares its deviance statistic
//! to a weighted sum `Q = Σ λᵢ·χ²₁` of unit chi-squares, where `λ` are the
//! eigenvalues of the design/naive covariance ratio ("misspecification"
//! factors in R). Two approximations are provided, matching R's
//! `method = "saddlepoint"` (default, with Satterthwaite fallback) and
//! `method = "satterthwaite"`:
//!
//! - **Satterthwaite**: match the first two moments of `Q`,
//!   `ndf = (Σλ)²/Σλ²`, `scale = Σλ²/Σλ`, then `P(Q ≥ x) ≈ P(χ²_ndf ≥ x/scale)`
//!   (F variant: `P(F ≥ x/(ndf·scale))` with `ddf` denominator df).
//! - **Saddlepoint** (Lugannani–Rice): for `Q = Σ aᵢ·Zᵢ²` with cumulant
//!   generating function `K(t) = −½ Σ ln(1 − 2 aᵢ t)`, solve `K'(ŝ) = x`,
//!   `w = sign(ŝ)·√(2(ŝx − K(ŝ)))`, `v = ŝ·√K''(ŝ)` and evaluate
//!   `P(Q ≥ x) ≈ Φ̄(w + ln(v/w)/w)`. Falls back to Satterthwaite when the
//!   root is degenerate (|ŝ| < 1e-4), exactly like R's `saddle()` returning
//!   `NA`.
//!
//! `pFsum`'s saddlepoint folds the random denominator into the quadratic
//! form: `F = Q_λ/q` vs `χ²_ddf/ddf` is rewritten as
//! `ΣλᵢZᵢ² − (x/ddf)·χ²_ddf ≥ 0`, so the coefficient vector becomes
//! `a' = (λ, −x/ddf × ddf)` and the tail is evaluated at 0.

use statrs::distribution::{ChiSquared, ContinuousCDF, FisherSnedecor, Normal};

/// Satterthwaite tail of `Q = Σ aᵢ χ²₁`: `P(Q ≥ x)`.
///
/// Requires all `a > 0` (same restriction as R's `"satterthwaite"` method).
pub fn pchisqsum_satterthwaite(x: f64, a: &[f64]) -> f64 {
    let (ndf, scale) = satterthwaite_moments(a);
    if !(ndf > 0.0 && scale > 0.0) {
        return f64::NAN;
    }
    ChiSquared::new(ndf)
        .map(|d| d.sf(x / scale))
        .unwrap_or(f64::NAN)
}

/// survey `pFsum` Satterthwaite branch: `P(F ≥ x)` with numerator
/// `Q_λ = Σ λᵢ χ²₁` and denominator `χ²_ddf / ddf`.
///
/// R (moment-matching on the numerator, exact denominator):
/// `tr = mean(λ)`, `tr2 = mean(λ²)/tr²`, `scale = tr·tr2`,
/// `ndf = len(λ)/tr2`, `p = pf(x/(ndf·scale), ndf, ddf)` — upper tail.
pub fn pfsum_satterthwaite(x: f64, lambda: &[f64], ddf: usize) -> f64 {
    let (ndf, scale) = satterthwaite_moments(lambda);
    if !(ndf > 0.0 && scale > 0.0) {
        return f64::NAN;
    }
    let ddf = ddf.max(1) as f64;
    FisherSnedecor::new(ndf, ddf)
        .map(|d| d.sf(x / (ndf * scale)))
        .unwrap_or(f64::NAN)
}

/// survey `pchisqsum` saddlepoint: `P(Q ≥ x)` for `Q = Σ λᵢ χ²₁`, refined by
/// Lugannani–Rice with Satterthwaite fallback.
pub fn pchisqsum_saddlepoint(x: f64, lambda: &[f64]) -> f64 {
    let sat = pchisqsum_satterthwaite(x, lambda);
    match saddle_upper(x, lambda) {
        Some(s) if s.is_finite() => s,
        _ => sat,
    }
}

/// survey `pFsum` saddlepoint: the statistic distribution is
/// `(ΣλZ²)/(χ²_ddf/ddf)`; the tail at `x` becomes `P(ΣλZ² − (x/ddf)χ²_ddf ≥ 0)`,
/// evaluated with the Lugannani–Rice saddle at 0. Satterthwaite fallback.
pub fn pfsum_saddlepoint(x: f64, lambda: &[f64], ddf: usize) -> f64 {
    let sat = pfsum_satterthwaite(x, lambda, ddf);
    if !x.is_finite() || x <= 0.0 || lambda.is_empty() || ddf == 0 {
        return sat;
    }
    let mut a: Vec<f64> = Vec::with_capacity(lambda.len() + ddf);
    a.extend_from_slice(lambda);
    let neg = -x / ddf as f64;
    for _ in 0..ddf {
        a.push(neg);
    }
    match saddle_upper(0.0, &a) {
        Some(s) if s.is_finite() => s.clamp(0.0, 1.0),
        _ => sat,
    }
}

// ── internals ──────────────────────────────────────────────────────────────

/// Moment matching shared by both Satterthwaite branches.
///
/// R: `tr = mean(a)`, `tr2 = mean(a²)/tr²`, returns `(ndf = n/tr2, scale = tr·tr2)`.
fn satterthwaite_moments(a: &[f64]) -> (f64, f64) {
    if a.is_empty() {
        return (f64::NAN, f64::NAN);
    }
    let n = a.len() as f64;
    let tr: f64 = a.iter().sum::<f64>() / n;
    let mean_sq: f64 = a.iter().map(|&v| v * v).sum::<f64>() / n;
    if tr <= 0.0 {
        return (f64::NAN, f64::NAN);
    }
    let tr2 = mean_sq / (tr * tr);
    (n / tr2, tr * tr2)
}

/// Cumulants of `Q = Σ aᵢ Zᵢ²`:
/// `K(t) = −½ Σ ln(1 − 2 aᵢ t)`,
/// `K'(t) = Σ aᵢ/(1 − 2 aᵢ t)`,
/// `K''(t) = Σ 2 aᵢ²/(1 − 2 aᵢ t)²`.

fn k_prime(a: &[f64], t: f64) -> f64 {
    a.iter()
        .map(|&ai| {
            let d = 1.0 - 2.0 * ai * t;
            if d.abs() < 1e-300 {
                f64::INFINITY * d.signum() * ai.signum()
            } else {
                ai / d
            }
        })
        .sum()
}

fn k_curly(a: &[f64], t: f64) -> f64 {
    a.iter()
        .map(|&ai| {
            let d = 1.0 - 2.0 * ai * t;
            2.0 * ai * ai / (d * d)
        })
        .sum()
}

fn k_cgf(a: &[f64], t: f64) -> f64 {
    -0.5 * a
        .iter()
        .map(|&ai| {
            let d = 1.0 - 2.0 * ai * t;
            if d <= 0.0 { f64::INFINITY } else { d.ln() }
        })
        .sum::<f64>()
}

/// Lugannani–Rice upper tail `P(Q ≥ x)` for `Q = Σ aᵢ Zᵢ²` (`a` may contain
/// negatives, e.g. pFsum's folded denominator). `None` ⇔ R's `saddle()`
/// returning `NA` (degenerate root), letting the caller fall back.
fn saddle_upper(x: f64, a: &[f64]) -> Option<f64> {
    if a.is_empty() || !x.is_finite() {
        return None;
    }
    let max_abs = a.iter().fold(0.0f64, |m, &v| m.max(v.abs()));
    if !(max_abs > 0.0) {
        return None; // Q ≡ 0
    }

    // Poles of K' at t = 1/(2aᵢ); the root lives in the interval between the
    // largest negative pole and the smallest positive pole. Following R's
    // saddle(), shrink the bracket to stay strictly inside the domain.
    let mut neg_pole = f64::NEG_INFINITY; // closest to 0 from the left
    let mut pos_pole = f64::INFINITY; // closest to 0 from the right
    for &ai in a {
        if ai != 0.0 {
            let p = 1.0 / (2.0 * ai);
            if p < 0.0 {
                neg_pole = neg_pole.max(p);
            } else {
                pos_pole = pos_pole.min(p);
            }
        }
    }

    // Bracket [lo, hi] with K'(lo) < x < K'(hi); double outward when an
    // endpoint is unbounded.
    let scale = 1.0 / (2.0 * max_abs);
    let mut lo = if neg_pole.is_finite() {
        neg_pole * (1.0 - 1e-12)
    } else {
        -scale
    };
    let mut hi = if pos_pole.is_finite() {
        pos_pole * (1.0 - 1e-12)
    } else {
        scale
    };
    let mut f_lo = k_prime(a, lo);
    if !f_lo.is_finite() {
        f_lo = f64::NEG_INFINITY;
    }
    let mut n_expand = 0;
    while f_lo >= x && n_expand < 200 {
        lo *= 2.0;
        f_lo = k_prime(a, lo);
        if !f_lo.is_finite() {
            f_lo = f64::NEG_INFINITY;
        }
        n_expand += 1;
    }
    if f_lo >= x {
        return None; // x below the attainable range (all-negative coefficients)
    }
    let mut f_hi = k_prime(a, hi);
    if !f_hi.is_finite() {
        f_hi = f64::INFINITY;
    }
    n_expand = 0;
    while f_hi <= x && n_expand < 200 {
        hi *= 2.0;
        f_hi = k_prime(a, hi);
        if !f_hi.is_finite() {
            f_hi = f64::INFINITY;
        }
        n_expand += 1;
    }
    if f_hi <= x {
        return None;
    }

    // Bisect to machine precision.
    for _ in 0..200 {
        let mid = 0.5 * (lo + hi);
        if mid == lo || mid == hi {
            break;
        }
        let f_mid = k_prime(a, mid);
        if !f_mid.is_finite() {
            break;
        }
        if f_mid < x {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    let zeta = 0.5 * (lo + hi);
    if zeta.abs() < 1e-4 {
        return None; // R: NA when |zeta| < 1e-4
    }

    let k_at = k_cgf(a, zeta);
    let kpp = k_curly(a, zeta);
    if !k_at.is_finite() || !(kpp > 0.0) {
        return None;
    }
    let arg = 2.0 * (zeta * x - k_at);
    if arg < 0.0 {
        return None;
    }
    let w = zeta.signum() * arg.sqrt();
    let v = zeta * kpp.sqrt();
    if !(w.abs() > 1e-12) || !(v > 0.0) {
        return None;
    }
    let z = w + (v / w).ln() / w;
    if !z.is_finite() {
        return None;
    }
    let n = Normal::new(0.0, 1.0).ok()?;
    Some(n.sf(z))
}

// ── Tests ──────────────────────────────────────────────────────────────────
//
// Golden values from R survey 4.5 (rocker/r-ver:4.5.1 probe,
// pFsum/pchisqsum with df = rep(1, length(lambda))): see
// stat_crates/survey/tests/svy_lrt_reference.rs for the full-fixture test.

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64, tol: f64) -> bool {
        (a - b).abs() <= tol
    }

    #[test]
    fn satterthwaite_matches_r_golden() {
        // probe: x=6, lambda=(1.4,0.9,0.7,1.1), ddf=30 → satterthwaite 0.23958314953571735
        let lam = [1.4, 0.9, 0.7, 1.1];
        assert!(close(
            pfsum_satterthwaite(6.0, &lam, 30),
            0.23958314953571735,
            1e-12
        ));
        // probe: x=12.5, lambda=(2.2,0.5,1.0), ddf=17 → 0.053554681057735073
        let lam = [2.2, 0.5, 1.0];
        assert!(close(
            pfsum_satterthwaite(12.5, &lam, 17),
            0.053554681057735073,
            1e-12
        ));
        // pchisqsum (ddf = Inf): x=6, lam=(1.4,0.9,0.7,1.1) → 0.2132808832123137
        let lam = [1.4, 0.9, 0.7, 1.1];
        assert!(close(
            pchisqsum_satterthwaite(6.0, &lam),
            0.2132808832123137,
            1e-12
        ));
    }

    #[test]
    fn saddlepoint_matches_r_golden() {
        // Probe pfsum/pchisqsum saddlepoint values (upper tail). Tolerance
        // 5e-6: R's saddle() roots via uniroot(tol=1e-8) on the d-rescaled
        // root, so near-degenerate cases (small |zeta|) only reproduce to
        // ~1e-6; our machine-precision bisection is the more accurate side.
        // The Satterthwaite goldens above pin the exact branch to 1e-12.
        assert!(close(
            pfsum_saddlepoint(6.0, &[1.4, 0.9, 0.7, 1.1], 30),
            0.23504147733823022,
            1e-7
        ));
        assert!(close(
            pfsum_saddlepoint(12.5, &[2.2, 0.5, 1.0], 17),
            0.051330076531508985,
            1e-7
        ));
        assert!(close(
            pfsum_saddlepoint(3.1, &[0.6; 5], 4),
            0.5010058794631097,
            5e-6
        ));
        assert!(close(
            pfsum_saddlepoint(9.9, &[1.0, 1.0], 33),
            0.013489955969101863,
            1e-7
        ));
        // probe pchisqsum saddlepoint values
        assert!(close(
            pchisqsum_saddlepoint(6.0, &[1.4, 0.9, 0.7, 1.1]),
            0.20795436126298045,
            1e-7
        ));
        assert!(close(
            pchisqsum_saddlepoint(12.5, &[2.2, 0.5, 1.0]),
            0.029876075579615066,
            1e-7
        ));
        assert!(close(
            pchisqsum_saddlepoint(3.1, &[0.6; 5]),
            0.3965184722876766,
            1e-7
        ));
    }

    #[test]
    fn equal_lambda_is_close_to_exact_f() {
        // lambda all equal ⇒ the pFsum statistic is 0.75·χ²₄/(χ²₂₀/20) =
        // 3·F(4, 20), so the exact tail is sf_F(4,20)(5/3). The
        // Lugannani–Rice formula is an approximation — loose agreement
        // only (exact parity is covered by the R goldens above).
        let lam = [0.75; 4];
        let p = pfsum_saddlepoint(5.0, &lam, 20);
        let exact = FisherSnedecor::new(4.0, 20.0)
            .map(|d| d.sf(5.0 / 3.0))
            .unwrap_or(f64::NAN);
        assert!((p - exact).abs() < 2e-3, "{p} vs {exact}");
    }

    #[test]
    fn tail_is_monotone_in_x() {
        let lam = [1.4, 0.9, 0.7, 1.1];
        let mut prev = 1.0;
        for &x in &[2.0, 4.0, 6.0, 9.0, 15.0] {
            let p = pfsum_saddlepoint(x, &lam, 25);
            assert!(p <= prev + 1e-12, "not decreasing at {x}");
            assert!((0.0..=1.0).contains(&p));
            prev = p;
        }
    }
}
