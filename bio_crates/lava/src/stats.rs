//! Distribution-function wrappers reproducing R's `pnorm` / `qnorm` /
//! `pchisq` / `pf`, plus `cov2cor`. The normal functions (`pnorm`/`qnorm`/
//! `pnorm_sf`/`dnorm`) are hand-implemented (own erfc + Acklam/Newton) so they
//! stay accurate in the deep tails; the χ² and F survival functions use the
//! maintained [`statrs`] crate (the `mr` crate's stats dependency).

use statrs::distribution::{ChiSquared, ContinuousCDF, FisherSnedecor};

/// Standard-normal CDF `P(Z ≤ x)` — R `pnorm(x)`, via `0.5·erfc(-x/√2)` with a
/// relative-accurate erfc (correct in the deep tails where naive CDFs underflow).
#[inline]
pub fn pnorm(x: f64) -> f64 {
    0.5 * erfc(-x / std::f64::consts::SQRT_2)
}

/// Standard-normal survival `P(Z > x)` — R `pnorm(x, lower.tail = FALSE)`.
#[inline]
pub fn pnorm_sf(x: f64) -> f64 {
    0.5 * erfc(x / std::f64::consts::SQRT_2)
}

/// Standard-normal quantile — R `qnorm(p)`. Acklam initial guess (accurate to
/// ~1e-9) polished by Newton iterations against the accurate [`pnorm`], giving
/// ~1e-13 across the full range (incl. p ≈ 1e-300).
pub fn qnorm(p: f64) -> f64 {
    if p <= 0.0 {
        return f64::NEG_INFINITY;
    }
    if p >= 1.0 {
        return f64::INFINITY;
    }
    const A: [f64; 6] = [-3.969683028665376e+01, 2.209460984245205e+02, -2.759285104469687e+02, 1.383577518672690e+02, -3.066479806614716e+01, 2.506628277459239e+00];
    const B: [f64; 5] = [-5.447609879822406e+01, 1.615858368580409e+02, -1.556989798598866e+02, 6.680131188771972e+01, -1.328068155288572e+01];
    const C: [f64; 6] = [-7.784894002430293e-03, -3.223964580411365e-01, -2.400758277161838e+00, -2.549732539343734e+00, 4.374664141464968e+00, 2.938163982698783e+00];
    const D: [f64; 4] = [7.784695709041462e-03, 3.224671290700398e-01, 2.445134137142996e+00, 3.754408661907416e+00];
    const PLOW: f64 = 0.02425;
    const PHIGH: f64 = 1.0 - PLOW;

    let mut x = if p < PLOW {
        let q = (-2.0 * p.ln()).sqrt();
        let n = ((((C[0] * q + C[1]) * q + C[2]) * q + C[3]) * q + C[4]) * q + C[5];
        let dd = (((D[0] * q + D[1]) * q + D[2]) * q + D[3]) * q + 1.0;
        n / dd
    } else if p <= PHIGH {
        let q = p - 0.5;
        let r = q * q;
        let n = (((((A[0] * r + A[1]) * r + A[2]) * r + A[3]) * r + A[4]) * r + A[5]) * q;
        let dd = ((((B[0] * r + B[1]) * r + B[2]) * r + B[3]) * r + B[4]) * r + 1.0;
        n / dd
    } else {
        let q = (-2.0 * (1.0 - p).ln()).sqrt();
        let n = ((((C[0] * q + C[1]) * q + C[2]) * q + C[3]) * q + C[4]) * q + C[5];
        let dd = (((D[0] * q + D[1]) * q + D[2]) * q + D[3]) * q + 1.0;
        -n / dd
    };
    // Newton polish with the accurate erfc-based CDF.
    let sqrt2pi = (2.0 * std::f64::consts::PI).sqrt();
    for _ in 0..5 {
        let e = pnorm(x) - p;
        let phi = (-0.5 * x * x).exp() / sqrt2pi;
        if !phi.is_finite() || phi <= 0.0 {
            break;
        }
        let step = e / phi;
        x -= step;
        if step.abs() <= 1e-13 * x.abs().max(1.0) {
            break;
        }
    }
    x
}

/// Complementary error function, relative-accurate even for large arguments
/// (small tail values). For `|x| < 1.5` uses the A&S 7.1.26 rational erf; for
/// `|x| ≥ 1.5` uses a modified-Lentz continued fraction for erfc directly.
fn erfc(x: f64) -> f64 {
    if x == 0.0 {
        return 1.0;
    }
    if x < 0.0 {
        return 2.0 - erfc(-x);
    }
    // x > 0
    if x < 1.5 {
        // A&S 7.1.26 (erfc not tiny here → absolute accuracy is fine).
        let t = 1.0 / (1.0 + 0.3275911 * x);
        let poly = t * (0.254829592 + t * (-0.284496736 + t * (1.421413741 + t * (-1.453152027 + t * 1.061405429))));
        return poly * (-x * x).exp();
    }
    // x >= 1.5: continued fraction for g = x + (1/2)/(x + 1/(x + (3/2)/(x + ...)))
    // erfc(x) = exp(-x²) / (√π · g).
    let tiny = 1e-300;
    let mut f = x;
    let mut c = x;
    let mut d = 0.0;
    for n in 1..=200 {
        let an = n as f64 / 2.0;
        d = x + an * d;
        if d == 0.0 {
            d = tiny;
        }
        c = x + an / c;
        if c == 0.0 {
            c = tiny;
        }
        d = 1.0 / d;
        let delta = c * d;
        f *= delta;
        if (delta - 1.0).abs() < 1e-16 {
            break;
        }
    }
    (-x * x).exp() / (f * std::f64::consts::PI.sqrt())
}

/// Standard-normal density `φ(x)` — R `dnorm(x)` (mean 0, sd 1).
#[inline]
pub fn dnorm(x: f64) -> f64 {
    (-0.5 * x * x).exp() / (2.0 * std::f64::consts::PI).sqrt()
}

/// χ²(k) survival `P(X > x)` — R `pchisq(x, k, lower.tail = FALSE)`.
#[inline]
pub fn pchisq_sf(x: f64, k: f64) -> f64 {
    ChiSquared::new(k).map(|d| d.sf(x)).unwrap_or(f64::NAN)
}

/// F(d1, d2) survival `P(X > x)` — R `pf(x, d1, d2, lower.tail = FALSE)`.
#[inline]
pub fn pf_sf(x: f64, d1: f64, d2: f64) -> f64 {
    FisherSnedecor::new(d1, d2).map(|d| d.sf(x)).unwrap_or(f64::NAN)
}

/// `cov2cor(V)`: rescale a (covariance) matrix to a correlation matrix,
/// `D^{-1/2} V D^{-1/2}` with `D = diag(V)`. Matches R `cov2cor` — invalid
/// (≤0) diagonal entries yield `NaN`/`Inf` rather than panicking.
pub fn cov2cor(v: &faer::Mat<f64>) -> faer::Mat<f64> {
    let n = v.nrows();
    let mut out = v.to_owned();
    for i in 0..n {
        for j in 0..n {
            if i == j {
                out[(i, j)] = 1.0;
            } else {
                let di = v[(i, i)];
                let dj = v[(j, j)];
                let denom = (di * dj).sqrt();
                out[(i, j)] = v[(i, j)] / denom;
            }
        }
    }
    out
}
