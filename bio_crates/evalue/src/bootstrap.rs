//! BCa (bias-corrected and accelerated) bootstrap for CI estimation.
//!
//! Replaces R's `boot::boot` + `boot.ci(type = "bca")` used in
//! `confounded_meta()` calibrated method.

use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;

/// BCa bootstrap CI result.
pub struct BcaResult {
    pub point: f64,
    pub lo: Option<f64>,
    pub hi: Option<f64>,
    pub se: f64,
    pub boot_vals: Vec<f64>,
}

/// Compute BCa bootstrap CI for a statistic.
///
/// - `data`: the original data (will be resampled)
/// - `statistic`: function(data_slice) → f64
/// - `r`: number of bootstrap iterations
/// - `ci_level`: confidence level (e.g. 0.95)
/// - `seed`: RNG seed for reproducibility
pub fn bca_ci<T, F>(data: &[T], statistic: F, r: usize, ci_level: f64, seed: u64) -> BcaResult
where
    T: Clone,
    F: Fn(&[T]) -> f64,
{
    let n = data.len();
    let mut rng = ChaCha8Rng::seed_from_u64(seed);
    let point = statistic(data);

    // Bootstrap replicates
    let mut boot_vals: Vec<f64> = Vec::with_capacity(r);
    for _ in 0..r {
        let mut sample: Vec<T> = Vec::with_capacity(n);
        for _ in 0..n {
            let idx = rng.random_range(0..n);
            sample.push(data[idx].clone());
        }
        let val = statistic(&sample);
        if val.is_finite() {
            boot_vals.push(val);
        }
    }

    if boot_vals.is_empty() {
        return BcaResult {
            point,
            lo: None,
            hi: None,
            se: f64::NAN,
            boot_vals: vec![],
        };
    }

    let se = {
        let mean: f64 = boot_vals.iter().sum::<f64>() / boot_vals.len() as f64;
        let var: f64 =
            boot_vals.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / boot_vals.len() as f64;
        var.sqrt()
    };

    // BCa acceleration parameter via jackknife
    let jack_vals: Vec<f64> = (0..n)
        .map(|i| {
            let mut jack_sample: Vec<T> = data.to_vec();
            jack_sample.remove(i);
            statistic(&jack_sample)
        })
        .filter(|v| v.is_finite())
        .collect();

    let a = if jack_vals.len() > 1 {
        let jack_mean: f64 = jack_vals.iter().sum::<f64>() / jack_vals.len() as f64;
        let num: f64 = jack_vals.iter().map(|v| (jack_mean - v).powi(3)).sum();
        let den: f64 = 6.0
            * (jack_vals
                .iter()
                .map(|v| (jack_mean - v).powi(2))
                .sum::<f64>())
            .powf(1.5);
        if den == 0.0 { 0.0 } else { num / den }
    } else {
        0.0
    };

    // Bias correction z0
    let count_less = boot_vals.iter().filter(|&&v| v < point).count();
    let prop_less = count_less as f64 / boot_vals.len() as f64;
    use statrs::distribution::{ContinuousCDF, Normal};
    let z = Normal::new(0.0, 1.0).unwrap();
    let z0 = if prop_less > 0.0 && prop_less < 1.0 {
        z.inverse_cdf(prop_less)
    } else {
        0.0 // edge case
    };

    // BCa quantiles
    let alpha = (1.0 - ci_level) / 2.0;
    let z_alpha = z.inverse_cdf(alpha);
    let z_1alpha = z.inverse_cdf(1.0 - alpha);

    let adj_lo = {
        let denom = 1.0 - a * (z0 + z_alpha);
        if denom.abs() < 1e-15 {
            alpha
        } else {
            let phi_arg = z0 + z_alpha / denom;
            z.cdf(phi_arg)
        }
    };

    let adj_hi = {
        let denom = 1.0 - a * (z0 + z_1alpha);
        if denom.abs() < 1e-15 {
            1.0 - alpha
        } else {
            let phi_arg = z0 + z_1alpha / denom;
            z.cdf(phi_arg)
        }
    };

    // Percentiles of boot_vals at adj_lo and adj_hi
    let mut sorted = boot_vals.clone();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

    let lo = percentile(&sorted, adj_lo);
    let hi = percentile(&sorted, adj_hi);

    BcaResult {
        point,
        lo,
        hi,
        se,
        boot_vals,
    }
}

fn percentile(sorted: &[f64], p: f64) -> Option<f64> {
    if sorted.is_empty() {
        return None;
    }
    let p = p.clamp(0.0, 1.0);
    let idx = p * (sorted.len() - 1) as f64;
    let lo = idx.floor() as usize;
    let hi = idx.ceil() as usize;
    if lo == hi {
        Some(sorted[lo])
    } else {
        let frac = idx - lo as f64;
        Some(sorted[lo] * (1.0 - frac) + sorted[hi] * frac)
    }
}
