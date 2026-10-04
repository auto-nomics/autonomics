//! Statistical distribution functions reproducing R's `pnorm`, `qnorm`,
//! `dnorm`, `qchisq`, `pchisq`.

use statrs::distribution::{ChiSquared, Continuous, ContinuousCDF, Normal};

/// Standard normal CDF: R's `pnorm(q)`.
pub fn pnorm(q: f64) -> f64 {
    Normal::standard().cdf(q)
}

/// Standard normal survival function: R's `pnorm(q, lower.tail = FALSE)`.
pub fn pnorm_sf(q: f64) -> f64 {
    1.0 - pnorm(q)
}

/// Two-sided normal tail: `2 * pnorm(-|z|)`.
pub fn pnorm_two_sided(z: f64) -> f64 {
    2.0 * pnorm(-z.abs())
}

/// Standard normal quantile: R's `qnorm(p)`.
pub fn qnorm(p: f64) -> f64 {
    Normal::standard().inverse_cdf(p)
}

/// Standard normal PDF: R's `dnorm(x)`.
pub fn dnorm(x: f64) -> f64 {
    Normal::standard().pdf(x)
}

/// χ² CDF with `df` degrees of freedom: R's `pchisq(q, df)`.
pub fn pchisq(q: f64, df: f64) -> f64 {
    if q <= 0.0 {
        return 0.0;
    }
    match ChiSquared::new(df) {
        Ok(dist) => dist.cdf(q),
        Err(_) => f64::NAN,
    }
}

/// χ² survival function accurate in the far tail.
///
/// Evaluates `Q(df/2, q/2)` (the regularized upper incomplete gamma, whose
/// continued-fraction branch is used exactly when the tail is small) instead
/// of `1 - cdf`: once the survival probability drops below ~1e-16 the
/// subtraction cancels to zero and every far-tail p-value would read as 0.
fn chi2_sf(q: f64, df: f64) -> f64 {
    if q.is_nan() || df.is_nan() {
        return f64::NAN;
    }
    if df <= 0.0 {
        return f64::NAN;
    }
    if q <= 0.0 {
        return 1.0;
    }
    if !q.is_finite() {
        return 0.0;
    }
    statrs::function::gamma::gamma_ur(df / 2.0, q / 2.0)
}

/// χ² survival function: R's `pchisq(q, df, lower.tail = FALSE)`.
pub fn pchisq_sf(q: f64, df: f64) -> f64 {
    chi2_sf(q, df)
}

/// χ² quantile: R's `qchisq(p, df)`.
pub fn qchisq(p: f64, df: f64) -> f64 {
    match ChiSquared::new(df) {
        Ok(dist) => dist.inverse_cdf(p),
        Err(_) => f64::NAN,
    }
}

/// χ² survival quantile: R's `qchisq(p, df, lower.tail = FALSE)`.
///
/// Inverts the survival function directly with bracketed bisection. The
/// naive `qchisq(1 - p, df)` formulation breaks down for tiny `p`: in
/// double precision `1 - p` rounds to exactly 1 once `p < ~1e-16`, and the
/// quantile degenerates to +∞. Root-finding on [`chi2_sf`] keeps the far
/// tail (down to p ≈ 1e-300) finite and accurate.
pub fn qchisq_sf(p: f64, df: f64) -> f64 {
    if p.is_nan() || df.is_nan() || df <= 0.0 {
        return f64::NAN;
    }
    if p <= 0.0 {
        return f64::INFINITY;
    }
    if p >= 1.0 {
        return 0.0;
    }
    // Bracket the root: sf(0) = 1 ≥ p, so grow `hi` until sf(hi) < p.
    let mut lo = 0.0f64;
    let mut hi = df.max(1.0);
    while chi2_sf(hi, df) >= p {
        if !hi.is_finite() {
            return f64::INFINITY;
        }
        lo = hi;
        hi *= 2.0;
    }
    // Bisect: 80 halvings shrink the bracket by ~2^-80, far past f64
    // resolution; the loop also stops once mid rounds onto an endpoint.
    for _ in 0..80 {
        let mid = 0.5 * (lo + hi);
        if mid <= lo || mid >= hi {
            break;
        }
        if chi2_sf(mid, df) >= p {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    0.5 * (lo + hi)
}

/// Median of a Chi-squared with 1 df, used by LDSC for λ_GC:
/// `qchisq(0.5, df = 1)` ≈ 0.4549.
pub fn median_chi2_1() -> f64 {
    qchisq(0.5, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pnorm() {
        assert!((pnorm(0.0) - 0.5).abs() < 1e-10);
        assert!((pnorm(1.96) - 0.975).abs() < 1e-3);
    }

    #[test]
    fn test_qnorm() {
        assert!((qnorm(0.5) - 0.0).abs() < 1e-10);
        assert!((qnorm(0.975) - 1.96).abs() < 1e-3);
    }

    #[test]
    fn test_pnorm_two_sided() {
        let z = 1.96;
        let p = pnorm_two_sided(z);
        assert!((p - 0.05).abs() < 1e-3);
    }

    #[test]
    fn test_pchisq() {
        assert!((pchisq(3.84, 1.0) - 0.95).abs() < 1e-2);
    }

    #[test]
    fn test_qchisq() {
        assert!((qchisq(0.95, 1.0) - 3.84).abs() < 1e-2);
    }

    /// Golden values from R 4.x `qchisq(p, df, lower.tail = FALSE)`,
    /// printed at full double precision (`sprintf("%.17g")`).
    #[test]
    fn qchisq_sf_matches_r_survival_quantile() {
        let cases: &[(f64, f64, f64)] = &[
            (1.0, 0.01, 6.6348966010212127),
            (1.0, 0.001, 10.827566170662729),
            (1.0, 1e-06, 23.928126976934827),
            (1.0, 1e-12, 50.844127911818148),
            (1.0, 1e-50, 224.38474831879654),
            (1.0, 1e-100, 453.94308223879898),
            (1.0, 1e-200, 913.76270078588061),
            (1.0, 1e-300, 1373.8726312223939),
            (2.0, 0.01, 9.2103403719761818),
            (2.0, 0.001, 13.815510557964274),
            (2.0, 1e-06, 27.631021115928547),
            (2.0, 1e-12, 55.262042231857095),
            (2.0, 1e-50, 230.25850929940458),
            (2.0, 1e-100, 460.51701859880916),
            (2.0, 1e-200, 921.03403719761832),
            (2.0, 1e-300, 1381.5510557964274),
            (3.5, 0.01, 12.329572300734482),
            (3.5, 0.001, 17.389858670332334),
            (3.5, 1e-06, 32.051284971150487),
            (3.5, 1e-12, 60.595988803129124),
            (3.5, 1e-50, 237.60607210879621),
            (3.5, 1e-100, 468.87799523980141),
            (3.5, 1e-200, 930.41979250933127),
            (3.5, 1e-300, 1391.5395402137831),
            (10.0, 0.01, 23.209251158954359),
            (10.0, 0.001, 29.588298445074418),
            (10.0, 1e-06, 46.863046846784385),
            (10.0, 1e-12, 78.471646562845237),
            (10.0, 1e-50, 262.99562096122946),
            (10.0, 1e-100, 498.33820041617952),
            (10.0, 1e-200, 964.11910061309823),
            (10.0, 1e-300, 1427.7719561298886),
            (79.0, 0.01, 111.14401942288376),
            (79.0, 0.001, 123.594365507585),
            (79.0, 1e-06, 153.70652642291878),
            (79.0, 1e-12, 201.95835453533064),
            (79.0, 1e-50, 435.58432104167696),
            (79.0, 1e-100, 702.48904219394194),
            (79.0, 1e-200, 1204.4189944903085),
            (79.0, 1e-300, 1691.0262613682917),
        ];
        for &(df, p, expected) in cases {
            let got = qchisq_sf(p, df);
            assert!(
                got.is_finite(),
                "qchisq_sf(p={p:e}, df={df}) = {got} is not finite (R: {expected})"
            );
            let rel = ((got - expected) / expected).abs();
            assert!(
                rel <= 1e-12,
                "qchisq_sf(p={p:e}, df={df}) = {got}, R = {expected}, rel err {rel:e}"
            );
        }
    }

    /// The old implementation computed `qchisq(1 - p, df)`, which rounds
    /// `1 - p` to exactly 1 for tiny p and returns +∞.
    #[test]
    fn qchisq_sf_stays_finite_in_the_far_tail() {
        for p in [1e-16, 1e-100, 1e-300, 4.9e-324] {
            let q = qchisq_sf(p, 1.0);
            assert!(q.is_finite(), "qchisq_sf({p:e}, 1) = {q}");
        }
        // Round-trip: sf(qchisq_sf(p)) ≈ p where sf itself is representable.
        for &(p, df) in &[(1e-2, 1.0), (1e-8, 3.5), (1e-50, 10.0)] {
            let q = qchisq_sf(p, df);
            let back = pchisq_sf(q, df);
            assert!(
                ((back - p) / p).abs() < 1e-9,
                "round-trip p={p:e} df={df}: sf({q}) = {back:e}"
            );
        }
        // Degenerate arguments mirror R's conventions.
        assert_eq!(qchisq_sf(1.0, 1.0), 0.0);
        assert_eq!(qchisq_sf(0.0, 1.0), f64::INFINITY);
        assert!(qchisq_sf(f64::NAN, 1.0).is_nan());
    }

    /// `pchisq_sf` must also stay accurate in the far tail now that it
    /// routes through the regularized upper incomplete gamma.
    #[test]
    fn pchisq_sf_keeps_far_tail_precision() {
        assert!((pchisq_sf(6.6348966010212127, 1.0) - 0.01).abs() < 1e-12);
        // R: pchisq(1373.8726312223939, 1, lower.tail=FALSE) = 1e-300.
        let sf = pchisq_sf(1373.8726312223939, 1.0);
        assert!(sf > 0.0, "far-tail sf must not cancel to zero, got {sf}");
        assert!((sf / 1e-300 - 1.0).abs() < 1e-6, "got {sf:e}");
        assert_eq!(pchisq_sf(0.0, 1.0), 1.0);
        assert_eq!(pchisq_sf(f64::INFINITY, 1.0), 0.0);
    }
}
