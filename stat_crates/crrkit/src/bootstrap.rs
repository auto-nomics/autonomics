//! Patient-level paired bootstrap for a difference of IPCW Brier scores.
//!
//! For two predictions of the same subjects (e.g. the frozen clinical model
//! `M0` and the fusion model `M3`), the SAP's primary comparison is
//! `ΔBS = BS(M0) − BS(M3)` with a 95% confidence interval from patient-level
//! bootstrap. Two details are non-negotiable and encoded here:
//!
//! * the resample is **paired** — both models are scored on exactly the same
//!   resampled patients, so the difference is within-patient;
//! * the censoring weights `G` are **re-estimated inside every resample**
//!   from the resampled data, per the frozen weighting plan — reusing the
//!   full-sample `G` would understate the uncertainty of the weighting
//!   itself.
//!
//! The interval is reported both as the percentile bootstrap and the basic
//! (reflected) bootstrap. Resamples whose censoring survival collapses below
//! the floor are counted as failures and skipped, never silently repaired;
//! the failure count is part of the result. The generator is seeded so a
//! frozen SAP can pin the interval.
//!
//! This interval quantifies the sampling variability of the **fixed frozen
//! models** in the validation population; it does not include the
//! model-development uncertainty (SAP §9.4).

use crate::brier::{BrierOptions, ipcw_brier};
use crate::error::{CrrkitError, Result};
use rand::Rng;
use rand::SeedableRng;
use rand_chacha::ChaCha8Rng;

/// Options for [`paired_brier_delta_bootstrap`].
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
#[serde(default)]
pub struct BootstrapOptions {
    /// Number of resamples; the SAP example uses 2000.
    pub n_boot: usize,
    /// Seed for the deterministic ChaCha8 generator.
    pub seed: u64,
    /// Significance level for the reported intervals (`alpha = 0.05` → 95%).
    pub alpha: f64,
}

impl Default for BootstrapOptions {
    fn default() -> Self {
        Self {
            n_boot: 2000,
            seed: 0,
            alpha: 0.05,
        }
    }
}

/// Result of the paired ΔBS bootstrap.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PairedBootstrapDelta {
    /// Point estimate `BS(model_a) − BS(model_b)` on the full sample.
    pub point_delta: f64,
    /// Full-sample Brier scores of the two models.
    pub brier_a: f64,
    /// Full-sample Brier score of model B.
    pub brier_b: f64,
    /// Percentile interval for ΔBS.
    pub ci_percentile: (f64, f64),
    /// Basic (reflected percentile) interval for ΔBS.
    pub ci_basic: (f64, f64),
    /// Two-sided percentile-bootstrap p-value for `H0: ΔBS = 0`, computed by
    /// the signed shift method (shift the statistic so the null holds, then
    /// measure the tail of the centred resampling distribution).
    pub p_value: f64,
    /// Resamples skipped because the in-resample censoring survival
    /// collapsed (or no outcome information remained).
    pub n_failed: usize,
    /// Usable resamples actually contributing to the intervals.
    pub n_usable: usize,
    /// The seed used, recorded for reproducibility.
    pub seed: u64,
}

/// Bootstrap the paired difference `BS(model_a) − BS(model_b)` at `horizon`.
///
/// A positive delta means model A (e.g. the clinical baseline `M0`) has the
/// **larger** prediction error, i.e. model B (e.g. the fusion `M3`) is better
/// — matching the SAP's `ΔBS = BS(M0) − BS(M3) > 0` success criterion.
pub fn paired_brier_delta_bootstrap(
    times: &[f64],
    status: &[u8],
    prob_a: &[f64],
    prob_b: &[f64],
    horizon: f64,
    brier_opts: &BrierOptions,
    boot_opts: &BootstrapOptions,
) -> Result<PairedBootstrapDelta> {
    crate::brier::validate_inputs(times, status, prob_a, horizon)?;
    crate::brier::validate_inputs(times, status, prob_b, horizon)?;
    if !(0.0..1.0).contains(&boot_opts.alpha) {
        return Err(CrrkitError::Invalid("alpha must lie in (0, 1)".to_string()));
    }
    if boot_opts.n_boot == 0 {
        return Err(CrrkitError::Invalid("n_boot must be positive".to_string()));
    }

    // Point estimates on the full sample.
    let full_a = ipcw_brier(times, status, prob_a, horizon, brier_opts)?;
    let full_b = ipcw_brier(times, status, prob_b, horizon, brier_opts)?;
    let point = full_a.brier - full_b.brier;

    let n = times.len();
    let mut rng = ChaCha8Rng::seed_from_u64(boot_opts.seed);
    let mut deltas: Vec<f64> = Vec::with_capacity(boot_opts.n_boot);
    let mut n_failed = 0usize;

    // Resampling workspace, reused across draws.
    let mut idx: Vec<usize> = vec![0; n];
    let mut t2: Vec<f64> = vec![0.0; n];
    let mut s2: Vec<u8> = vec![0; n];
    let mut pa2: Vec<f64> = vec![0.0; n];
    let mut pb2: Vec<f64> = vec![0.0; n];

    for _ in 0..boot_opts.n_boot {
        for slot in idx.iter_mut() {
            *slot = rng.random_range(0..n);
        }
        for j in 0..n {
            t2[j] = times[idx[j]];
            s2[j] = status[idx[j]];
            pa2[j] = prob_a[idx[j]];
            pb2[j] = prob_b[idx[j]];
        }
        let ok = match (
            ipcw_brier(&t2, &s2, &pa2, horizon, brier_opts),
            ipcw_brier(&t2, &s2, &pb2, horizon, brier_opts),
        ) {
            (Ok(ra), Ok(rb)) => {
                deltas.push(ra.brier - rb.brier);
                true
            }
            // Collapsed censoring or no outcome information in this draw:
            // count and skip, never repair.
            _ => false,
        };
        if !ok {
            n_failed += 1;
        }
    }

    let n_usable = deltas.len();
    if n_usable == 0 {
        return Err(CrrkitError::BootstrapExhausted {
            n_boot: boot_opts.n_boot,
        });
    }

    deltas.sort_by(|a, b| a.total_cmp(b));
    let lo_q = quantile_sorted(&deltas, boot_opts.alpha / 2.0);
    let hi_q = quantile_sorted(&deltas, 1.0 - boot_opts.alpha / 2.0);
    let ci_basic = (2.0 * point - hi_q, 2.0 * point - lo_q);

    // Shifted-null p-value: centre the resampling distribution on the null
    // (delta = 0) by removing the point estimate, then double the smaller
    // one-sided tail.
    let centred: Vec<f64> = deltas.iter().map(|&d| d - point).collect();
    let mean_centred = centred.iter().sum::<f64>() / centred.len() as f64;
    let beyond = centred
        .iter()
        .filter(|&&d| (d - mean_centred).abs() >= (-point).abs())
        .count() as f64;
    let p_value = (2.0 * beyond / n_usable as f64).min(1.0);

    Ok(PairedBootstrapDelta {
        point_delta: point,
        brier_a: full_a.brier,
        brier_b: full_b.brier,
        ci_percentile: (lo_q, hi_q),
        ci_basic,
        p_value,
        n_failed,
        n_usable,
        seed: boot_opts.seed,
    })
}

/// Linear-interpolated quantile of an ascending sorted sample (type 7).
fn quantile_sorted(sorted: &[f64], q: f64) -> f64 {
    let n = sorted.len();
    if n == 1 {
        return sorted[0];
    }
    let pos = q * (n - 1) as f64;
    let lo = pos.floor() as usize;
    let hi = (lo + 1).min(n - 1);
    let frac = pos - lo as f64;
    sorted[lo] * (1.0 - frac) + sorted[hi] * frac
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_models_give_delta_zero() {
        let times = [200.0, 300.0, 400.0, 500.0, 1500.0, 1600.0];
        let status = [1u8, 1, 2, 2, 0, 1];
        let prob = [0.7, 0.6, 0.3, 0.2, 0.4, 0.5];
        let r = paired_brier_delta_bootstrap(
            &times,
            &status,
            &prob,
            &prob,
            1095.75,
            &BrierOptions::default(),
            &BootstrapOptions {
                n_boot: 200,
                seed: 7,
                ..BootstrapOptions::default()
            },
        )
        .unwrap();
        assert!(r.point_delta.abs() < 1e-12);
        assert!(r.ci_percentile.0.abs() < 1e-12 && r.ci_percentile.1.abs() < 1e-12);
        assert_eq!(r.n_usable, 200);
    }

    #[test]
    fn better_model_yields_positive_delta_for_baseline_minus_fusion() {
        // model_a is anti-calibrated, model_b matches the outcomes exactly.
        let times = [200.0, 300.0, 400.0, 500.0, 1500.0, 1600.0];
        let status = [1u8, 1, 2, 2, 0, 0];
        let prob_a = [0.1, 0.2, 0.8, 0.9, 0.7, 0.6];
        let prob_b = [0.9, 0.8, 0.2, 0.1, 0.3, 0.4];
        let r = paired_brier_delta_bootstrap(
            &times,
            &status,
            &prob_a,
            &prob_b,
            1095.75,
            &BrierOptions::default(),
            &BootstrapOptions {
                n_boot: 500,
                seed: 11,
                ..BootstrapOptions::default()
            },
        )
        .unwrap();
        assert!(r.point_delta > 0.0);
        assert!(r.ci_percentile.0 > 0.0);
    }

    #[test]
    fn deterministic_under_seed() {
        let times = [100.0, 300.0, 700.0, 1200.0];
        let status = [1u8, 2, 0, 1];
        let pa = [0.6, 0.4, 0.3, 0.7];
        let pb = [0.5, 0.5, 0.4, 0.6];
        let o = BootstrapOptions {
            n_boot: 100,
            seed: 42,
            alpha: 0.05,
        };
        let r1 = paired_brier_delta_bootstrap(
            &times,
            &status,
            &pa,
            &pb,
            1095.75,
            &BrierOptions::default(),
            &o,
        )
        .unwrap();
        let r2 = paired_brier_delta_bootstrap(
            &times,
            &status,
            &pa,
            &pb,
            1095.75,
            &BrierOptions::default(),
            &o,
        )
        .unwrap();
        assert_eq!(r1.ci_percentile, r2.ci_percentile);
    }

    #[test]
    fn quantile_interpolates() {
        let x = [1.0, 2.0, 3.0, 4.0];
        assert!((quantile_sorted(&x, 0.0) - 1.0).abs() < 1e-12);
        assert!((quantile_sorted(&x, 0.5) - 2.5).abs() < 1e-12);
        assert!((quantile_sorted(&x, 1.0) - 4.0).abs() < 1e-12);
    }
}
