//! Bootstrap resampling and confidence intervals for survey-weighted analyses.
//!
//! Shared infrastructure for the weighted mediation family
//! ([`crate::mediation_weighted`], [`crate::mediation_serial`],
//! [`crate::mediation_moderated`]):
//!
//! - [`percentile_ci`] / [`bias_corrected_ci`] / [`ci`] — interval
//!   construction from a vector of bootstrap replicates. The bias-corrected
//!   (BC) interval adjusts for median bias in the bootstrap distribution
//!   (Efron 1987), matching the `boot.ci(type = "bca")`-adjacent BC flavour
//!   reported by `mediation`/`RMediation` for survey-weighted effects.
//! - [`BootstrapDesign`] — resampling scheme. With only `weights` it performs
//!   ordinary row-level resampling; with `strata` (+ optional `psu`) it
//!   resamples PSUs with replacement within strata, the with-replacement
//!   bootstrap appropriate for complex-survey designs (NHANES SDMVSTRA /
//!   SDMVPSU). Weights ride along with their rows unscaled.
//!
//! The name `BootstrapDesign` is deliberate — the survey-design object of
//! the `survey` crate is a different concept (linearization variance), and
//! the two must not be confused.

use std::collections::BTreeMap;

use rand::Rng;
use rand_chacha::ChaCha8Rng;
use statrs::distribution::{ContinuousCDF, Normal};

use crate::error::{EpiError, Result};

/// Two-sided Φ⁻¹(0.975), the normal quantile behind 95% intervals.
const Z_975: f64 = 1.959963984540054;

// ── CI methods ──────────────────────────────────────────────────────────────

/// Bootstrap confidence-interval construction method.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CiMethod {
    /// Ordinary percentile interval: empirical 2.5% / 97.5% quantiles of the
    /// bootstrap distribution.
    Percentile,
    /// Bias-corrected interval: corrects for the median bias of the
    /// bootstrap distribution before reading the percentile bounds.
    BiasCorrected,
}

/// Empirical 2.5% / 97.5% percentile interval from bootstrap replicates.
///
/// Index convention: `floor` on the lower bound, `ceil` on the upper, both
/// clamped into `0..n-1`. Fewer than two replicates yield `(NaN, NaN)`.
pub fn percentile_ci(boot: &[f64]) -> (f64, f64) {
    quantiles(boot, 0.025, 0.975)
}

/// Bias-corrected (BC) 95% interval (Efron 1987).
///
/// `z0 = Φ⁻¹(p0)` where `p0` is the proportion of replicates at or below the
/// point estimate; bounds sit at the `Φ(2z0 ∓ z_.975)` quantiles of the
/// bootstrap distribution. `p0` is clamped to `[1/(2B), 1 − 1/(2B)]` so an
/// all-one-sided replicate set degrades gracefully toward the shifted
/// percentile interval instead of producing infinite quantile levels.
pub fn bias_corrected_ci(boot: &[f64], estimate: f64) -> (f64, f64) {
    let n = boot.len();
    if n < 2 {
        return (f64::NAN, f64::NAN);
    }
    let n_le = boot.iter().filter(|&&b| b <= estimate).count();
    let p0 = (n_le as f64 / n as f64).clamp(1.0 / (2.0 * n as f64), 1.0 - 1.0 / (2.0 * n as f64));
    let normal = Normal::standard();
    let z0 = normal.inverse_cdf(p0);
    let alpha_lo = normal.cdf(2.0 * z0 - Z_975);
    let alpha_hi = normal.cdf(2.0 * z0 + Z_975);
    quantiles(boot, alpha_lo, alpha_hi)
}

/// Interval by the requested [`CiMethod`].
pub fn ci(method: CiMethod, boot: &[f64], estimate: f64) -> (f64, f64) {
    match method {
        CiMethod::Percentile => percentile_ci(boot),
        CiMethod::BiasCorrected => bias_corrected_ci(boot, estimate),
    }
}

/// Sorted-quantile bounds at `alpha_lo` / `alpha_hi`, indices clamped in-range.
fn quantiles(boot: &[f64], alpha_lo: f64, alpha_hi: f64) -> (f64, f64) {
    if boot.len() < 2 {
        return (f64::NAN, f64::NAN);
    }
    let mut sorted = boot.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let n = sorted.len();
    let lo_idx = ((alpha_lo * n as f64).floor() as usize).min(n - 1);
    let hi_idx = ((alpha_hi * n as f64).ceil() as usize).min(n - 1);
    (sorted[lo_idx], sorted[hi_idx])
}

// ── Resampling design ───────────────────────────────────────────────────────

/// Bootstrap resampling scheme for (optionally survey-weighted) data.
///
/// Not a variance-linearization design — see the module docs. The borrowed
/// slices must outlive the design; all rows are resampled from the original
/// arrays, so consumers index into their own columns with the returned
/// replicate indices.
#[derive(Debug, Clone, Copy)]
pub struct BootstrapDesign<'a> {
    /// Sampling weights, strictly positive and finite. Carried unchanged
    /// with their rows into each replicate (with-replacement semantics — no
    /// Rao-Wu rescaling in v1).
    pub weights: &'a [f64],
    /// Stratum codes per row; interpreted jointly with `psu`.
    pub strata: Option<&'a [u64]>,
    /// PSU (cluster) codes per row, interpreted within stratum.
    pub psu: Option<&'a [u64]>,
}

impl<'a> BootstrapDesign<'a> {
    /// Row-level resampling design (weights only, no clustering).
    pub fn new(weights: &'a [f64]) -> Result<Self> {
        Self::with_clusters(weights, None, None)
    }

    /// Cluster-aware design. `psu` without `strata` is rejected — PSU codes
    /// are only meaningful within a stratum. `strata` without `psu`
    /// degenerates to row-level resampling stratified by `strata`.
    pub fn with_clusters(
        weights: &'a [f64],
        strata: Option<&'a [u64]>,
        psu: Option<&'a [u64]>,
    ) -> Result<Self> {
        let n = weights.len();
        if n == 0 {
            return Err(EpiError::EmptyInput);
        }
        for (i, &w) in weights.iter().enumerate() {
            if !w.is_finite() || w <= 0.0 {
                return Err(EpiError::Numerical(format!(
                    "weight at position {i} must be finite and positive"
                )));
            }
        }
        if let Some(s) = strata {
            if s.len() != n {
                return Err(EpiError::DimensionMismatch { a: n, b: s.len() });
            }
        }
        if let Some(p) = psu {
            if p.len() != n {
                return Err(EpiError::DimensionMismatch { a: n, b: p.len() });
            }
            if strata.is_none() {
                return Err(EpiError::Numerical(
                    "psu requires strata (cluster codes are interpreted within stratum)".to_string(),
                ));
            }
        }
        Ok(Self { weights, strata, psu })
    }

    /// Number of rows the design was built from.
    pub fn n(&self) -> usize {
        self.weights.len()
    }

    /// Weights rescaled to mean 1 — numerically friendlier for the WLS
    /// normal equations (coefficients of a linear WLS fit are invariant to
    /// weight scaling, so this changes nothing statistically).
    pub fn normalized_weights(&self) -> Vec<f64> {
        let mean = self.weights.iter().sum::<f64>() / self.weights.len() as f64;
        self.weights.iter().map(|&w| w / mean).collect()
    }

    /// Draw one replicate as a vector of row indices into the original data.
    ///
    /// * no strata — `n` rows drawn i.i.d. with replacement;
    /// * strata only — within each stratum, `n_h` rows drawn with replacement
    ///   from that stratum;
    /// * strata + psu — within each stratum, `n_psu_h` distinct PSUs drawn
    ///   with replacement and every row of each drawn PSU enters the
    ///   replicate, so the replicate length can exceed `n`.
    ///
    /// Strata are visited in ascending code order, making the draw
    /// reproducible for a given RNG state.
    pub fn replicate_indices(&self, rng: &mut ChaCha8Rng) -> Vec<usize> {
        let n = self.n();
        match (self.strata, self.psu) {
            (None, _) => draw_range(rng, n, n),
            (Some(strata), None) => {
                let rows_by_stratum: BTreeMap<u64, Vec<usize>> =
                    strata.iter().enumerate().fold(BTreeMap::new(), |mut m, (i, &h)| {
                        m.entry(h).or_default().push(i);
                        m
                    });
                let mut out = Vec::with_capacity(n);
                for rows in rows_by_stratum.values() {
                    out.extend(draw_from(rng, rows, rows.len()));
                }
                out
            }
            (Some(strata), Some(psu)) => {
                // stratum -> psu -> rows, both keyed ascending.
                let mut by_stratum: BTreeMap<u64, BTreeMap<u64, Vec<usize>>> = BTreeMap::new();
                for (i, (&h, &c)) in strata.iter().zip(psu).enumerate() {
                    by_stratum.entry(h).or_default().entry(c).or_default().push(i);
                }
                let mut out = Vec::with_capacity(n);
                for psus in by_stratum.values() {
                    let psu_ids: Vec<u64> = psus.keys().copied().collect();
                    for drawn in draw_from(rng, &psu_ids, psu_ids.len()) {
                        out.extend_from_slice(&psus[&drawn]);
                    }
                }
                out
            }
        }
    }
}

/// `k` i.i.d. draws with replacement from `0..k`.
fn draw_range(rng: &mut ChaCha8Rng, k: usize, draws: usize) -> Vec<usize> {
    (0..draws)
        .map(|_| (rng.random::<f64>() * k as f64) as usize)
        .collect()
}

/// `draws` i.i.d. draws with replacement from the items of `pool`.
fn draw_from<T: Copy>(rng: &mut ChaCha8Rng, pool: &[T], draws: usize) -> Vec<T> {
    (0..draws)
        .map(|_| pool[(rng.random::<f64>() * pool.len() as f64) as usize])
        .collect()
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use rand::SeedableRng;

    use super::*;

    #[test]
    fn bc_matches_percentile_when_symmetric() {
        // Symmetric replicate set around the estimate: z0 = 0, BC == percentile.
        let boot: Vec<f64> = (-50..50).map(|i| i as f64 + 0.5).collect();
        let (plo, phi) = percentile_ci(&boot);
        let (blo, bhi) = bias_corrected_ci(&boot, 0.0);
        assert_eq!(plo, blo);
        assert_eq!(phi, bhi);
    }

    #[test]
    fn bc_shifts_interval_when_estimate_below_all_replicates() {
        let boot: Vec<f64> = (0..200).map(|i| 10.0 + i as f64).collect();
        let (plo, _) = percentile_ci(&boot);
        let (blo, _) = bias_corrected_ci(&boot, 0.0);
        // All replicates above the estimate → correction pushes bounds down.
        assert!(blo < plo, "bc lower {blo} should sit below percentile lower {plo}");
    }

    #[test]
    fn degenerate_replicate_sets_do_not_panic() {
        let (lo, hi) = percentile_ci(&[]);
        assert!((lo.is_nan(), hi.is_nan()) == (true, true));
        let (lo, hi) = bias_corrected_ci(&[1.0], 1.0);
        assert!((lo.is_nan(), hi.is_nan()) == (true, true));
        // Two replicates exercise the index clamp (ceil(0.975·2) = 2 → oob).
        let (lo, hi) = percentile_ci(&[3.0, 1.0]);
        assert_eq!((lo, hi), (1.0, 3.0));
    }

    #[test]
    fn ci_dispatch_matches_direct_calls() {
        let boot: Vec<f64> = (0..50).map(|i| i as f64).collect();
        assert_eq!(ci(CiMethod::Percentile, &boot, 25.0), percentile_ci(&boot));
        assert_eq!(ci(CiMethod::BiasCorrected, &boot, 25.0), bias_corrected_ci(&boot, 25.0));
    }

    #[test]
    fn same_seed_same_replicate() {
        let w = [1.0; 16];
        let d = BootstrapDesign::new(&w).unwrap();
        let a = d.replicate_indices(&mut ChaCha8Rng::seed_from_u64(7));
        let b = d.replicate_indices(&mut ChaCha8Rng::seed_from_u64(7));
        let c = d.replicate_indices(&mut ChaCha8Rng::seed_from_u64(8));
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert!(a.iter().all(|&i| i < 16));
    }

    #[test]
    fn psu_replicate_preserves_stratum_row_counts() {
        // 2 strata × 2 PSUs × 5 rows: each replicate draws 2 PSUs per stratum,
        // every drawn PSU contributes all 5 of its rows.
        let n = 20;
        let w = vec![100.0; n];
        let mut strata = vec![0u64; 10];
        strata.extend(vec![1u64; 10]);
        let mut psu = vec![0u64; 5];
        psu.extend(vec![1u64; 5]);
        psu.extend(vec![0u64; 5]);
        psu.extend(vec![1u64; 5]);
        let d = BootstrapDesign::with_clusters(&w, Some(&strata), Some(&psu)).unwrap();
        assert_eq!(d.n(), n);

        let idx = d.replicate_indices(&mut ChaCha8Rng::seed_from_u64(42));
        assert_eq!(idx.len(), n);
        let in_s0 = idx.iter().filter(|&&i| strata[i] == 0).count();
        let in_s1 = idx.iter().filter(|&&i| strata[i] == 1).count();
        assert_eq!((in_s0, in_s1), (10, 10));
        // Each PSU enters 0/1/2 times → its rows appear in blocks of 5·k.
        for c in [0u64, 1] {
            let cnt: usize = idx.iter().filter(|&&i| psu[i] == c && strata[i] == 0).count();
            assert_eq!(cnt % 5, 0);
        }
    }

    #[test]
    fn normalized_weights_average_one() {
        let w = [2000.0, 150_000.0, 80_000.0, 45_000.0];
        let d = BootstrapDesign::new(&w).unwrap();
        let nw = d.normalized_weights();
        let mean = nw.iter().sum::<f64>() / nw.len() as f64;
        assert!((mean - 1.0).abs() < 1e-12);
        assert!(nw.iter().all(|&v| v.is_finite() && v > 0.0));
    }

    #[test]
    fn rejects_invalid_designs() {
        assert!(BootstrapDesign::new(&[]).is_err());
        assert!(BootstrapDesign::new(&[1.0, 0.0]).is_err());
        assert!(BootstrapDesign::new(&[1.0, f64::NAN]).is_err());
        assert!(BootstrapDesign::new(&[-1.0, 1.0]).is_err());
        // Length mismatches.
        assert!(BootstrapDesign::with_clusters(&[1.0, 1.0], Some(&[0]), None).is_err());
        assert!(BootstrapDesign::with_clusters(&[1.0, 1.0], Some(&[0, 1]), Some(&[0])).is_err());
        // psu without strata.
        assert!(BootstrapDesign::with_clusters(&[1.0, 1.0], None, Some(&[0, 1])).is_err());
    }
}
