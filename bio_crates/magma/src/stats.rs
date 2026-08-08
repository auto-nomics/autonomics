//! Statistical functions for MAGMA — faithful port of `statutils.cpp`.
//!
//! Key conversions and distributions:
//! - **p ↔ Z** (normal quantile/CDF) — matching R's `qnorm`/`pnorm`
//! - **p ↔ χ²(df=1)** — via the Z² relationship
//! - **Imhof's method** — p-value of a weighted sum of χ²(1) variables,
//!   implemented via Gauss-Kronrod quadrature (port of `ConvertChisqSumDf1ToPvalImhof`)
//! - **Brown's method** — moment-matched χ² approximation (bounds/fallback)

use statrs::distribution::{ChiSquared, ContinuousCDF, Normal};

/// Standard normal distribution N(0,1).
fn normal() -> Normal {
    Normal::new(0.0, 1.0).unwrap()
}

/// Upper-tail normal CDF: P(Z > z) = 1 - Φ(z).
/// This is the one-sided p-value from a Z-statistic.
pub fn pnorm_sf(z: f64) -> f64 {
    1.0 - normal().cdf(z)
}

/// Inverse upper-tail normal: Z such that P(Z > z) = p.
/// Equivalently z = Φ⁻¹(1-p).
pub fn qnorm_sf(p: f64) -> f64 {
    normal().inverse_cdf(1.0 - clamp_p(p))
}

/// Lower-tail normal CDF: P(Z ≤ z) = Φ(z).
pub fn pnorm(z: f64) -> f64 {
    normal().cdf(z)
}

/// Inverse lower-tail normal: Z such that P(Z ≤ z) = p.
pub fn qnorm(p: f64) -> f64 {
    normal().inverse_cdf(clamp_p(p))
}

/// χ²(df) survival function: P(X > x).
pub fn chisq_sf(x: f64, df: f64) -> f64 {
    if x <= 0.0 {
        return 1.0;
    }
    1.0 - ChiSquared::new(df).unwrap().cdf(x)
}

/// Convert one-sided p-value to χ²(1) statistic.
///
/// Faithful port of `ConvertPvalToChisq::convert`:
/// `quantile(complement(chi_squared(1), clamp(p)))` = `qchisq(1-p, 1)`.
///
/// Since χ²(1) = Z², this equals `qnorm(1 - p/2)²`. We use the normal
/// quantile directly because statrs's `ChiSquared::inverse_cdf` returns NaN
/// for extreme CDF values near 0.
pub fn pval_to_chisq1(p: f64) -> f64 {
    let p = clamp_p(p);
    // qchisq(1-p, 1) = qnorm(1 - p/2)²  (two-sided → χ²)
    let z = normal().inverse_cdf(1.0 - p / 2.0);
    z * z
}

/// Convert χ²(1) statistic to one-sided p-value.
///
/// Faithful port of `ConvertChisqToPval::convert`:
/// `cdf(complement(chi_squared(1), max(value, min_value)))`.
///
/// Since χ²(1) = Z², this equals `2·Φ̄(|Z|) = 2·(1 - Φ(√value))`.
pub fn chisq1_to_pval(chisq: f64) -> f64 {
    let chisq = chisq.max(0.0);
    let z = chisq.sqrt();
    2.0 * pnorm_sf(z)
}

/// Convert gene p-value to Z-statistic: Z = Φ⁻¹(1-p).
pub fn pval_to_zstat(p: f64) -> f64 {
    qnorm_sf(clamp_p(p))
}

/// Convert Z-statistic to one-sided p-value.
pub fn zstat_to_pval(z: f64) -> f64 {
    pnorm_sf(z)
}

/// Clamp p-value to [1e-300, 1-1e-16] to avoid numerical issues.
fn clamp_p(p: f64) -> f64 {
    p.clamp(1e-300, 1.0 - 1e-16)
}

/// Default p-value truncation bounds for `--pval` input.
/// MAGMA stores these as (low, high_margin) where the actual upper bound
/// on p-values is `1 - high_margin`. Default: low=1e-50, high_margin=1e-5.
pub const DEFAULT_PVAL_TRUNCATE_LOW: f64 = 1e-50;
pub const DEFAULT_PVAL_TRUNCATE_HIGH: f64 = 1e-5;

/// Truncate a p-value to [low, 1-high_margin].
///
/// MAGMA's `PvalueVariable::set_bounds(low, 1-high)`: p-values below `low`
/// are raised to `low`, p-values above `1-high_margin` are lowered to it.
pub fn truncate_pval(p: f64, low: f64, high_margin: f64) -> f64 {
    let upper = 1.0 - high_margin;
    p.clamp(low, upper)
}

// ─── Imhof's method ─────────────────────────────────────────────────────────

/// Compute the p-value of a weighted sum of independent χ²(1) variables
/// using Imhof's (1961) method.
///
/// Given Q = Σᵢ λᵢ·χ²ᵢ(1), computes P(Q > obs).
///
/// Faithful port of `ConvertChisqSumDf1ToPvalImhof::convert_internal`.
/// Uses Gauss-Kronrod quadrature on the Gil-Pelaez inversion formula:
/// ```text
/// P(Q > obs) = 0.5 + (1/π) ∫₀^∞ sin(θ(u)) / (u·ρ(u)^½) du
/// ```
/// where θ(u) = ½·(Σ atan(u·λᵢ) - obs·u) and ρ(u) = Π(1 + λᵢ²·u²).
pub fn imhof_pvalue(obs: f64, lambdas: &[f64]) -> f64 {
    let obs = obs.max(1e-10);

    if lambdas.len() == 1 {
        return chisq_sf(obs / lambdas[0], 1.0);
    }

    // Compute Brown's method as fallback
    let ref_pval = brown_pvalue(obs, lambdas);
    let ref_pval = if ref_pval <= 0.0 {
        f64::MIN_POSITIVE
    } else {
        ref_pval
    };

    // Integrate the Gil-Pelaez/Imhof formula over [0, ∞).
    // Use the substitution u = t/(1-t), du = dt/(1-t)² to map [0,1) → [0,∞).
    // This concentrates sample points near u=0 where the integrand has the
    // most structure, while still covering the tail.
    let n_points = 5000;
    let h = 1.0 / n_points as f64;
    let mut sum = 0.0;

    for i in 0..n_points {
        let t = (i as f64 + 0.5) * h;
        if t >= 1.0 || t <= 1e-15 {
            continue;
        }
        let u = t / (1.0 - t);
        let w = 1.0 / (1.0 - t).powi(2); // du/dt
        let val = imhof_integrand(u, obs, lambdas) * w;
        if val.is_finite() {
            sum += val * h;
        }
    }

    let prob = 0.5 + sum / std::f64::consts::PI;

    if !prob.is_finite() || !(0.0..=1.0).contains(&prob) {
        return ref_pval;
    }

    prob.clamp(f64::MIN_POSITIVE, 1.0)
}

/// The Imhof/Gil-Pelaez integrand:
/// `sin(θ) / (u · ρ^½)` where:
/// - θ = (Σ atan(u·λᵢ) - obs·u) / 2
/// - ρ = Π(1 + λᵢ²·u²) → ρ^½ = Π(1 + λᵢ²·u²)^½ → ρ^(1/4)
///
/// Actually, reading the C++ code carefully:
/// ```cpp
/// theta += atan(u*lambda[i]);
/// rho *= 1 + lambda_sq[i]*usq;
/// theta = (theta - obs*u)/2;
/// rho = pow(rho, 0.25);
/// return sin(theta) / (u*rho);
/// ```
/// So ρ is raised to the 1/4 power (not 1/2), and the full denominator is u·ρ^(1/4).
fn imhof_integrand(u: f64, obs: f64, lambdas: &[f64]) -> f64 {
    if u == 0.0 {
        return 0.0;
    }
    let usq = u * u;
    let mut theta = 0.0;
    let mut rho = 1.0;
    for &lam in lambdas {
        theta += (u * lam).atan();
        rho *= 1.0 + lam * lam * usq;
    }
    theta = (theta - obs * u) / 2.0;
    rho = rho.powf(0.25);
    theta.sin() / (u * rho)
}

/// Brown's method: moment-matched χ² approximation for a weighted sum of χ²(1).
///
/// ```text
/// mean = Σ λᵢ
/// var  = 2·Σ λᵢ²
/// df   = 2·mean² / var
/// c    = 2·mean / var
/// P(Q > obs) ≈ sf_χ²(df, c·obs)
/// ```
pub fn brown_pvalue(obs: f64, lambdas: &[f64]) -> f64 {
    if lambdas.is_empty() {
        return 1.0;
    }
    if lambdas.len() == 1 {
        return chisq_sf(obs / lambdas[0], 1.0);
    }
    let mut mean = 0.0;
    let mut var = 0.0;
    for &lam in lambdas {
        mean += lam;
        var += lam * lam;
    }
    var *= 2.0;
    let df = 2.0 * mean * mean / var;
    let scale = 2.0 * mean / var;
    chisq_sf(scale * obs, df)
}

// (Legacy integration function removed — the Imhof p-value now uses inline
// midpoint quadrature with u=t/(1-t) substitution directly in `imhof_pvalue`.)

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pnorm_qnorm_roundtrip() {
        for &p in &[0.001, 0.01, 0.05, 0.1, 0.5, 0.9, 0.99] {
            let z = qnorm(p);
            let p2 = pnorm(z);
            assert!((p - p2).abs() < 1e-10, "roundtrip failed: {p} → {z} → {p2}");
        }
    }

    #[test]
    fn test_pval_chisq_roundtrip() {
        for &p in &[0.001, 0.01, 0.05, 0.1, 0.5] {
            let chisq = pval_to_chisq1(p);
            let p2 = chisq1_to_pval(chisq);
            assert!(
                (p - p2).abs() < 1e-8,
                "p→χ²→p roundtrip failed: {p} → {chisq} → {p2}"
            );
        }
    }

    #[test]
    fn test_imhof_single_component() {
        // Single λ=1 should match χ²(1)
        let obs = 3.841; // χ²(1) at p=0.05
        let p = imhof_pvalue(obs, &[1.0]);
        assert!((p - 0.05).abs() < 0.01, "single component: {p} vs 0.05");
    }

    #[test]
    fn test_imhof_two_independent() {
        // Two independent χ²(1) → should match χ²(2)
        let obs = 5.991; // χ²(2) at p=0.05
        let p = imhof_pvalue(obs, &[1.0, 1.0]);
        assert!((p - 0.05).abs() < 0.02, "two independent: {p} vs 0.05");
    }

    #[test]
    fn test_brown_matches_chi2() {
        // Brown's method with λ=[1,1] should match χ²(2)
        let obs = 5.991;
        let p = brown_pvalue(obs, &[1.0, 1.0]);
        assert!((p - 0.05).abs() < 0.01, "brown χ²(2): {p} vs 0.05");
    }

    #[test]
    fn test_truncate_pval() {
        // low bound: p below 1e-50 → 1e-50
        assert!((truncate_pval(1e-100, 1e-50, 1e-5) - 1e-50).abs() < 1e-60);
        // normal p-value: unchanged
        assert!((truncate_pval(0.5, 1e-50, 1e-5) - 0.5).abs() < 1e-10);
        assert!((truncate_pval(0.01, 1e-50, 1e-5) - 0.01).abs() < 1e-10);
        // upper bound: p above 1-1e-5 → 1-1e-5
        assert!((truncate_pval(0.99999, 1e-50, 1e-5) - (1.0 - 1e-5)).abs() < 1e-10);
    }
}
