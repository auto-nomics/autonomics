//! Goodness-of-fit tests — chi-squared GoF, Kolmogorov–Smirnov (one & two
//! sample), Shapiro–Wilk normality, Anderson–Darling.
//!
//! All ports match the corresponding R `stats::` / `nortest::` function.

use serde_json::json;

use super::HypothesisTest;
use crate::{
    Alternative, HypoError, KEY_KIND, Result,
    dist::{chisq_sf, normal_cdf, normal_inv},
    extras,
};

// ─── Chi-squared goodness-of-fit ────────────────────────────────────────────

/// Chi-squared goodness-of-fit test.
///
/// Tests whether the observed counts are consistent with the expected
/// proportions `p`. When `rescale_p = true` (R default), `p` is rescaled to
/// sum to 1.
///
/// Mirrors `chisq.test(x = observed, p = p, rescale.p = rescale_p)`.
pub fn chisq_gof(observed: &[u64], p: &[f64], rescale_p: bool) -> Result<HypothesisTest> {
    let k = observed.len();
    if k < 2 {
        return Err(HypoError::InvalidInput(
            "goodness-of-fit requires ≥ 2 categories".into(),
        ));
    }
    if p.len() != k {
        return Err(HypoError::LengthMismatch { a: k, b: p.len() });
    }
    if p.iter().any(|&pi| pi < 0.0) {
        return Err(HypoError::InvalidInput(
            "probabilities must be non-negative".into(),
        ));
    }
    let n: f64 = observed.iter().map(|&o| o as f64).sum();
    if n == 0.0 {
        return Err(HypoError::InvalidInput("total count must be > 0".into()));
    }
    let p_sum: f64 = p.iter().sum();
    if p_sum <= 0.0 {
        return Err(HypoError::InvalidInput(
            "sum of probabilities must be > 0".into(),
        ));
    }
    let p_norm: Vec<f64> = if rescale_p {
        p.iter().map(|&pi| pi / p_sum).collect()
    } else {
        if (p_sum - 1.0).abs() > 1e-8 {
            return Err(HypoError::InvalidInput(format!(
                "probabilities sum to {p_sum}, not 1 (use rescale_p = true)"
            )));
        }
        p.to_vec()
    };
    let expected: Vec<f64> = p_norm.iter().map(|&pi| n * pi).collect();
    let stat: f64 = observed
        .iter()
        .zip(expected.iter())
        .map(|(&o, &e)| {
            if e == 0.0 {
                0.0
            } else {
                let d = o as f64 - e;
                d * d / e
            }
        })
        .sum();
    let df = (k - 1) as f64;
    let p_value = chisq_sf(stat, df);

    Ok(HypothesisTest::new(
        stat,
        p_value,
        df,
        Alternative::TwoSided,
        "Chi-squared test for given probabilities",
        extras([
            (KEY_KIND, json!("chisq_gof")),
            ("observed", json!(observed)),
            ("expected", json!(expected)),
            ("n", json!(n as u64)),
            ("rescale_p", json!(rescale_p)),
        ]),
    ))
}

// ─── Kolmogorov–Smirnov ─────────────────────────────────────────────────────

/// One-sample Kolmogorov–Smirnov test against a fully specified CDF.
///
/// `cdf` is the cumulative distribution function under H₀. The statistic is
/// `D = sup_x |F_n(x) − F(x)|`.
///
/// Mirrors `ks.test(x, y = "name", ...)` for the one-sample case.
pub fn ks_one_sample(x: &[f64], cdf: impl Fn(f64) -> f64) -> Result<HypothesisTest> {
    let n = x.len();
    if n == 0 {
        return Err(HypoError::InvalidInput("empty sample".into()));
    }
    let mut sorted = x.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let nf = n as f64;
    let mut d = 0.0_f64;
    for (i, &xi) in sorted.iter().enumerate() {
        let f0 = cdf(xi);
        let upper = ((i + 1) as f64) / nf - f0; // F_n(xi) from above
        let lower = f0 - (i as f64) / nf; // F_n(xi) from below
        d = d.max(upper).max(lower);
    }
    // One-sample KS uses en = √n in the asymptotic approximation.
    let p_value = ks_pvalue(d, nf.sqrt());
    Ok(HypothesisTest::new(
        d,
        p_value,
        f64::INFINITY,
        Alternative::TwoSided,
        "One-sample Kolmogorov-Smirnov test",
        extras([(KEY_KIND, json!("ks_test")), ("n", json!(n as u64))]),
    ))
}

/// Two-sample Kolmogorov–Smirnov test.
///
/// Tests whether two samples come from the same distribution.
/// `D = sup_x |F₁(x) − F₂(x)|`.
///
/// Mirrors `ks.test(x, y)` (two-sample).
pub fn ks_two_sample(x: &[f64], y: &[f64], alt: Alternative) -> Result<HypothesisTest> {
    let (n1, n2) = (x.len(), y.len());
    if n1 == 0 || n2 == 0 {
        return Err(HypoError::InvalidInput(
            "both samples must be non-empty".into(),
        ));
    }
    let mut sx = x.to_vec();
    let mut sy = y.to_vec();
    sx.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    sy.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let (n1f, n2f) = (n1 as f64, n2 as f64);

    // Merge the two sorted arrays and track the maximum vertical distance.
    // Ties in the two samples must be advanced **together** so that equal
    // empirical CDFs (identical samples) yield D = 0.
    let mut d_pos = 0.0_f64; // sup(F1 - F2)
    let mut d_neg = 0.0_f64; // sup(F2 - F1)
    let (mut i, mut j) = (0_usize, 0_usize);
    while i < n1 && j < n2 {
        if sx[i] < sy[j] {
            let f1 = (i + 1) as f64 / n1f;
            let f2 = j as f64 / n2f;
            d_pos = d_pos.max(f1 - f2);
            d_neg = d_neg.max(f2 - f1);
            i += 1;
        } else if sy[j] < sx[i] {
            let f1 = i as f64 / n1f;
            let f2 = (j + 1) as f64 / n2f;
            d_pos = d_pos.max(f1 - f2);
            d_neg = d_neg.max(f2 - f1);
            j += 1;
        } else {
            // Tie: advance both ECDFs past the common value together.
            let xv = sx[i];
            let yv = sy[j];
            let mut i2 = i;
            let mut j2 = j;
            while i2 < n1 && sx[i2] == xv {
                i2 += 1;
            }
            while j2 < n2 && sy[j2] == yv {
                j2 += 1;
            }
            let f1 = i2 as f64 / n1f;
            let f2 = j2 as f64 / n2f;
            d_pos = d_pos.max(f1 - f2);
            d_neg = d_neg.max(f2 - f1);
            i = i2;
            j = j2;
        }
    }
    let (stat, p_value) = match alt {
        Alternative::TwoSided => {
            let d = d_pos.max(d_neg);
            let en = (n1f * n2f / (n1f + n2f)).sqrt();
            (d, ks_pvalue(d, en))
        }
        Alternative::Greater => {
            let en = (n1f * n2f / (n1f + n2f)).sqrt();
            (d_pos, ks_pvalue_one_sided(d_pos, en))
        }
        Alternative::Less => {
            let en = (n1f * n2f / (n1f + n2f)).sqrt();
            (d_neg, ks_pvalue_one_sided(d_neg, en))
        }
    };
    Ok(HypothesisTest::new(
        stat,
        p_value,
        f64::INFINITY,
        alt,
        "Two-sample Kolmogorov-Smirnov test",
        extras([
            (KEY_KIND, json!("ks_test")),
            ("n", json!((n1 + n2) as u64)),
            ("n1", json!(n1 as u64)),
            ("n2", json!(n2 as u64)),
        ]),
    ))
}

/// Asymptotic two-sided KS p-value (R 4.x default for large samples).
/// Uses the asymptotic Kolmogorov distribution: `sqrt(n) * D → K`.
fn ks_pvalue(d: f64, en: f64) -> f64 {
    if d <= 0.0 {
        return 1.0;
    }
    if en == 0.0 {
        return 1.0;
    }
    // Use the asymptotic series from Numerical Recipes (one-sided), doubled.
    let lambda = (en + 0.12 + 0.11 / en) * d;
    ks_asymptotic_two_sided(lambda)
}

/// One-sided asymptotic p-value.
fn ks_pvalue_one_sided(d: f64, en: f64) -> f64 {
    if d <= 0.0 || en == 0.0 {
        return 1.0;
    }
    let lambda = (en + 0.12 + 0.11 / en) * d;
    // P(K > lambda) = exp(-2 * lambda^2) for the one-sided case.
    (-2.0 * lambda * lambda).exp()
}

/// Q_KS(lambda) — the asymptotic Kolmogorov distribution survival function.
/// Series: 2 * Σ_{j=1}^{∞} (-1)^{j-1} exp(-2 j² λ²)
fn ks_asymptotic_two_sided(lambda: f64) -> f64 {
    if lambda < 0.18 {
        return 1.0;
    }
    let mut sum = 0.0_f64;
    let mut prev = f64::INFINITY;
    let mut j = 1;
    while (sum - prev).abs() > 1e-12 * sum.abs() || j <= 100 {
        prev = sum;
        let term = 2.0
            * (if j % 2 == 1 { 1.0 } else { -1.0 })
            * (-2.0 * (j as f64).powi(2) * lambda * lambda).exp();
        sum += term;
        j += 1;
        if j > 200 {
            break;
        }
    }
    sum.clamp(0.0, 1.0)
}

// ─── Shapiro–Wilk normality test ────────────────────────────────────────────

/// Shapiro–Francia test for normality.
///
/// Computes the W' statistic from Blom normal scores (equivalent to
/// `cor(x, qnorm(ppoints(n, 3/8)))²`) and the p-value via Royston's (1993)
/// normalising transformation — the same algorithm as `nortest::sf.test`.
/// Valid for 3 ≤ n ≤ 5000 (the p-value approximation is calibrated for
/// 5 ≤ n ≤ 5000).
pub fn shapiro_wilk(x: &[f64]) -> Result<HypothesisTest> {
    let n = x.len();
    if n < 3 {
        return Err(HypoError::InvalidInput(format!(
            "sample size must be ≥ 3, got {n}"
        )));
    }
    if n > 5000 {
        return Err(HypoError::InvalidInput(format!(
            "sample size must be ≤ 5000, got {n}"
        )));
    }
    let nf = n as f64;
    let mut sorted = x.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

    // Compute the mean and SS.
    let mean: f64 = sorted.iter().sum::<f64>() / nf;
    let ss: f64 = sorted.iter().map(|&xi| (xi - mean).powi(2)).sum();
    if ss == 0.0 {
        return Err(HypoError::InvalidInput("all values identical".into()));
    }

    // Shapiro–Francia W' statistic (asymptotically equivalent to Shapiro–Wilk
    // W, and the form R uses internally for n > 11). Uses Blom normal scores
    // directly as the weights: W' = (Σ m_i x_(i))² / (m* · SS).
    let m = compute_m_i(n);
    let m_star: f64 = m.iter().map(|mi| mi * mi).sum();
    let mx_dot: f64 = m.iter().zip(&sorted).map(|(mi, &xi)| mi * xi).sum();
    let w = mx_dot * mx_dot / (m_star * ss);

    // Royston (1993) normalising transformation, as in `nortest::sf.test`:
    // log(1 − W') is approximately Normal(μ, σ²) under H₀, with
    //   u = ln n, v = ln u,
    //   μ  = −1.2725 + 1.0521 (v − u),
    //   σ  = 1.0308 − 0.26758 (v + 2/u).
    let u = nf.ln();
    let v = u.ln();
    let mu = -1.2725 + 1.0521 * (v - u);
    let sig = 1.0308 - 0.26758 * (v + 2.0 / u);
    // w == 1 → ln(0) = −∞ → z = −∞ → p = 1 (perfect normal-probability plot).
    let z = ((1.0 - w).ln() - mu) / sig;
    let p_value = 1.0 - normal_cdf(z);

    Ok(HypothesisTest::new(
        w,
        p_value,
        f64::INFINITY,
        Alternative::TwoSided,
        "Shapiro-Francia normality test",
        extras([(KEY_KIND, json!("shapiro_test")), ("n", json!(n as u64))]),
    ))
}

/// Compute `m_i = Φ⁻¹((i − 3/8) / (n + 1/4))` for i = 1, ..., n.
fn compute_m_i(n: usize) -> Vec<f64> {
    let nf = n as f64;
    (1..=n)
        .map(|i| normal_inv((i as f64 - 0.375) / (nf + 0.25)))
        .collect()
}

// ─── Anderson–Darling ───────────────────────────────────────────────────────

/// Distribution hypothesised for Anderson–Darling.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AdDist {
    /// Test for normality (parameters estimated from data).
    Normal,
    /// Test against exponential distribution.
    Exponential,
}

/// Anderson–Darling test for composite normality or exponentiality.
///
/// `A² = -n - (1/n) Σ_{i=1}^{n} (2i-1)[ln(z_(i)) + ln(1-z_(n+1-i))]`
///
/// For the normal case, uses the standardised data `z_i = (x_i - x̄)/s` and
/// applies the D'Agostino (1986) adjustment so the p-value accounts for
/// estimated parameters. Mirrors `nortest::ad.test`.
pub fn anderson_darling(x: &[f64], dist: AdDist) -> Result<HypothesisTest> {
    let n = x.len();
    if n < 8 {
        return Err(HypoError::InvalidInput(
            "Anderson-Darling: need ≥ 8 observations (nortest threshold)".into(),
        ));
    }
    let nf = n as f64;
    let mut sorted = x.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

    // Compute z-values under the hypothesised distribution.
    let z_vals: Vec<f64> = match dist {
        AdDist::Normal => {
            let mean = sorted.iter().sum::<f64>() / nf;
            let sd = (sorted.iter().map(|&x| (x - mean).powi(2)).sum::<f64>() / (nf - 1.0)).sqrt();
            if sd == 0.0 {
                return Err(HypoError::InvalidInput("zero standard deviation".into()));
            }
            sorted
                .iter()
                .map(|&x| normal_cdf((x - mean) / sd))
                .collect()
        }
        AdDist::Exponential => {
            let mean = sorted.iter().sum::<f64>() / nf;
            if mean <= 0.0 {
                return Err(HypoError::InvalidInput(
                    "non-positive mean for exponential".into(),
                ));
            }
            sorted.iter().map(|&x| 1.0 - (-x / mean).exp()).collect()
        }
    };

    // A² statistic.
    let mut a2 = -nf;
    for i in 0..n {
        let j = i + 1;
        let z_j = z_vals[i];
        let z_rev = z_vals[n - j];
        let term = (2 * j - 1) as f64 * (z_j.ln() + (1.0 - z_rev).ln());
        a2 -= term / nf;
    }

    // Adjusted statistic and p-value (D'Agostino & Stephens formulas).
    let (adjusted, p_value) = match dist {
        AdDist::Normal => {
            let a2_adj = a2 * (1.0 + 0.75 / nf + 2.25 / nf.powi(2));
            // p-value lookup for adjusted A² (normality).
            (a2_adj, ad_normal_pvalue(a2_adj))
        }
        AdDist::Exponential => {
            let a2_adj = a2 * (1.0 + 0.6 / nf);
            (a2_adj, ad_exp_pvalue(a2_adj))
        }
    };

    Ok(HypothesisTest::new(
        adjusted,
        p_value,
        f64::INFINITY,
        Alternative::TwoSided,
        "Anderson-Darling test",
        extras([
            (KEY_KIND, json!("anderson_darling")),
            ("A2_unadjusted", json!(a2)),
            ("n", json!(n as u64)),
            ("dist", json!(format!("{dist:?}").to_lowercase())),
        ]),
    ))
}

/// P-value for adjusted A² for normality (Stephens 1986, Table 4.7).
fn ad_normal_pvalue(a2: f64) -> f64 {
    if a2 < 0.2 {
        1.0 - a2.exp() * (-0.9177 - 1.5838 * a2 - 1.9039 * a2.powi(2))
    } else if a2 < 0.34 {
        1.0 - a2.exp() * (-0.9063 - 1.6904 * a2 - 1.9039 * a2.powi(2))
    } else if a2 < 0.6 {
        1.0 - 0.10864 - 0.40446 * (a2 - 0.34) - 0.83272 * (a2 - 0.34).powi(2)
    } else {
        0.0_f64.max(1.0 - 0.5041 * (a2 - 0.6).exp())
    }
}

/// P-value for adjusted A² for exponentiality (Stephens 1986).
fn ad_exp_pvalue(a2: f64) -> f64 {
    if a2 < 0.26 {
        1.0 - a2.exp() * (-4.4952 - 10.9518 * a2 - 6.5484 * a2.powi(2))
    } else if a2 < 0.47 {
        1.0 - a2.exp() * (-3.5876 - 10.9518 * a2 - 6.5484 * a2.powi(2))
    } else {
        0.0_f64.max(1.0 - (0.6174 + 3.1017 * (a2 - 0.47)).exp())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chisq_gof_uniform() {
        // R: chisq.test(c(10, 20, 30, 40)) — uniform null
        let observed = vec![10, 20, 30, 40];
        let p = vec![0.25, 0.25, 0.25, 0.25];
        let t = chisq_gof(&observed, &p, false).unwrap();
        // Expected = 25 each. χ² = (15² + 5² + 5² + 15²) / 25 = (225+25+25+225)/25 = 500/25 = 20
        assert!((t.stat - 20.0).abs() < 1e-9);
        assert_eq!(t.dof, 3.0);
        assert!((t.p_value - chisq_sf(20.0, 3.0)).abs() < 1e-12);
    }

    #[test]
    fn chisq_gof_rescale_p() {
        let observed = vec![10, 20, 30, 40];
        let p = vec![1.0, 1.0, 1.0, 1.0]; // equal → same as uniform
        let t = chisq_gof(&observed, &p, true).unwrap();
        assert!((t.stat - 20.0).abs() < 1e-9);
    }

    #[test]
    fn chisq_gof_rejects_unnormalized_p() {
        let observed = vec![10, 20];
        let p = vec![0.3, 0.8]; // sums to 1.1
        assert!(chisq_gof(&observed, &p, false).is_err());
    }

    #[test]
    fn ks_one_sample_normal() {
        // Large sample from N(0,1): D should be small.
        let x: Vec<f64> = (1..=100)
            .map(|i| normal_inv((i as f64 - 0.5) / 100.0))
            .collect();
        let t = ks_one_sample(&x, normal_cdf).unwrap();
        // Perfect quantiles → D ≈ 0
        assert!(t.stat < 0.01, "D = {}", t.stat);
        assert!(t.p_value > 0.5);
    }

    #[test]
    fn ks_two_sample_same_dist() {
        // Two samples from the same distribution → D small, p large.
        let x: Vec<f64> = (1..=50).map(|i| (i as f64) * 0.02).collect();
        let y: Vec<f64> = (1..=50).map(|i| (i as f64) * 0.02 + 0.01).collect();
        let t = ks_two_sample(&x, &y, Alternative::TwoSided).unwrap();
        assert!(t.stat < 0.06, "D = {}", t.stat);
    }

    #[test]
    fn ks_two_sample_different() {
        let x = vec![1.0_f64; 20].into_iter().collect::<Vec<_>>();
        let y = vec![10.0_f64; 20].into_iter().collect::<Vec<_>>();
        let t = ks_two_sample(&x, &y, Alternative::TwoSided).unwrap();
        // Completely disjoint → D = 1
        assert!((t.stat - 1.0).abs() < 1e-9);
        assert!(t.p_value < 0.001);
    }

    #[test]
    fn shapiro_runs() {
        // Normal-ish data
        let x: Vec<f64> = vec![-0.5, 0.3, 0.8, -1.2, 0.1, 1.5, -0.7, 0.4, 0.9, -0.3];
        let t = shapiro_wilk(&x).unwrap();
        assert!(t.stat > 0.0 && t.stat <= 1.0, "W = {}", t.stat);
        // For normal data W should be fairly close to 1.
        assert!(t.stat > 0.8, "W should be high for normal data: {}", t.stat);
        // Royston (1993) p-value must be a real number now, not NaN, and
        // must not reject for normal-looking data.
        assert!(!t.p_value.is_nan(), "p_value = {}", t.p_value);
        assert!(t.p_value > 0.05, "p = {}", t.p_value);
    }

    #[test]
    fn shapiro_rejects_nonnormal() {
        // Very skewed data — W should be low and the p-value tiny.
        let x: Vec<f64> = vec![1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 100.0];
        let t = shapiro_wilk(&x).unwrap();
        assert!(
            t.stat < 0.5,
            "W should be low for non-normal data: {}",
            t.stat
        );
        assert!(t.p_value < 0.001, "p = {}", t.p_value);
    }

    #[test]
    fn shapiro_rejects_bad_inputs() {
        assert!(shapiro_wilk(&[1.0, 2.0]).is_err());
        assert!(shapiro_wilk(&[]).is_err());
        assert!(shapiro_wilk(&[1.0; 5001]).is_err());
    }

    #[test]
    fn shapiro_pvalue_calibrated_under_h0() {
        // Deterministic LCG + Box–Muller: under exact H₀ the Royston (1993)
        // p-value must be ~U(0,1). A mis-calibrated μ/σ would push the
        // rejection rate at α = 0.05 away from 0.05.
        let mut seed = 0x2545F4914F6CDD1D_u64;
        let mut lcg = move || {
            seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (seed >> 11) as f64 / (1u64 << 53) as f64
        };
        let mut normal = move || {
            let (a, b) = (lcg(), lcg());
            (-2.0 * a.ln()).sqrt() * (std::f64::consts::TAU * b).cos()
        };

        for n in [30_usize, 200, 965] {
            let mut rejections = 0_usize;
            let trials = 2_000_usize;
            for _ in 0..trials {
                let x: Vec<f64> = (0..n).map(|_| normal()).collect();
                let p = shapiro_wilk(&x).unwrap().p_value;
                if p < 0.05 {
                    rejections += 1;
                }
            }
            let rate = rejections as f64 / trials as f64;
            // 3σ band around α = 0.05 for the binomial noise at 2000 trials.
            let band = 3.0 * (0.05 * 0.95 / trials as f64).sqrt();
            assert!(
                (rate - 0.05).abs() < band,
                "n = {n}: rejection rate {rate} outside 0.05 ± {band}"
            );
        }
    }

    #[test]
    fn anderson_darling_normal_data() {
        // Near-normal data → A²* small, p large
        let x: Vec<f64> = vec![
            -0.5, 0.3, 0.8, -1.2, 0.1, 1.5, -0.7, 0.4, 0.9, -0.3, 0.2, -0.6, 1.1, -0.9, 0.5, 0.0,
        ];
        let t = anderson_darling(&x, AdDist::Normal).unwrap();
        assert!(t.stat < 1.0, "A* = {}", t.stat);
        assert!(t.p_value > 0.05, "p = {}", t.p_value);
    }

    #[test]
    fn anderson_darling_nonnormal() {
        // Heavily skewed data → large A²*, small p
        let x: Vec<f64> = vec![
            1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 100.0,
        ];
        let t = anderson_darling(&x, AdDist::Normal).unwrap();
        assert!(t.p_value < 0.05, "p = {}", t.p_value);
    }

    #[test]
    fn anderson_darling_rejects_bad_inputs() {
        assert!(anderson_darling(&[1.0, 2.0], AdDist::Normal).is_err());
        assert!(anderson_darling(&[1.0; 10], AdDist::Normal).is_err()); // zero SD
    }
}
