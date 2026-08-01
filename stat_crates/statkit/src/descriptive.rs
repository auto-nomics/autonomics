//! Descriptive statistics: central tendency, dispersion, quantiles, ranks.
//!
//! All functions take borrowed `&[f64]` slices and are zero-allocation except
//! where sorting is unavoidable ([`quantile`], [`rank`]). Compensated
//! (Neumaier) summation is used internally for numerically stable mean /
//! variance computation.
//!
//! Conventions:
//! - Empty input → [`StatError::EmptyInput`].
//! - "Variance" / "std dev" default to the **sample** form (`n−1` denominator).
//!   Population variants (`n`) are suffixed `_population`.
//! - Quantiles use linear interpolation (R type 7 / NumPy default).
//! - Ranks use the **average** method for ties (R default, SciPy `'average'`).

use crate::error::{Result, StatError};

// ── Summation ──────────────────────────────────────────────────────────────

/// Compensated (Neumaier / Kahan–Babuška) summation.
fn compensated_sum(iter: impl IntoIterator<Item = f64>) -> f64 {
    let mut sum = 0.0_f64;
    let mut c = 0.0_f64;
    for value in iter {
        let t = sum + value;
        if sum.abs() > value.abs() {
            c += (sum - t) + value;
        } else {
            c += (value - t) + sum;
        }
        sum = t;
    }
    sum + c
}

// ── Central tendency ───────────────────────────────────────────────────────

/// Sum of values (compensated).
pub fn sum(xs: &[f64]) -> Result<f64> {
    if xs.is_empty() {
        return Err(StatError::EmptyInput);
    }
    Ok(compensated_sum(xs.iter().copied()))
}

/// Arithmetic mean.
pub fn mean(xs: &[f64]) -> Result<f64> {
    if xs.is_empty() {
        return Err(StatError::EmptyInput);
    }
    Ok(compensated_sum(xs.iter().copied()) / xs.len() as f64)
}

// ── Dispersion ─────────────────────────────────────────────────────────────

fn variance_ddof(xs: &[f64], ddof: usize) -> Result<f64> {
    let n = xs.len();
    if n <= ddof {
        return Err(StatError::InsufficientData {
            min: ddof + 1,
            actual: n,
        });
    }
    let mu = compensated_sum(xs.iter().copied()) / n as f64;
    let ss = compensated_sum(xs.iter().map(|&x| {
        let d = x - mu;
        d * d
    }));
    Ok(ss / (n - ddof) as f64)
}

/// Sample variance (`n−1` denominator).
pub fn variance(xs: &[f64]) -> Result<f64> {
    variance_ddof(xs, 1)
}

/// Population variance (`n` denominator).
pub fn variance_population(xs: &[f64]) -> Result<f64> {
    variance_ddof(xs, 0)
}

/// Sample standard deviation.
pub fn std_dev(xs: &[f64]) -> Result<f64> {
    Ok(variance(xs)?.sqrt())
}

/// Population standard deviation.
pub fn std_dev_population(xs: &[f64]) -> Result<f64> {
    Ok(variance_population(xs)?.sqrt())
}

// ── Covariance / correlation ───────────────────────────────────────────────

/// Sample covariance (`n−1`).
pub fn covariance(xs: &[f64], ys: &[f64]) -> Result<f64> {
    let n = xs.len();
    if n != ys.len() {
        return Err(StatError::LengthMismatch { a: n, b: ys.len() });
    }
    if n <= 1 {
        return Err(StatError::InsufficientData { min: 2, actual: n });
    }
    let mx = compensated_sum(xs.iter().copied()) / n as f64;
    let my = compensated_sum(ys.iter().copied()) / n as f64;
    let cov = compensated_sum(xs.iter().zip(ys).map(|(&x, &y)| (x - mx) * (y - my)));
    Ok(cov / (n - 1) as f64)
}

/// Pearson correlation coefficient.
///
/// Errors if either variable has zero variance.
pub fn correlation(xs: &[f64], ys: &[f64]) -> Result<f64> {
    let n = xs.len();
    if n != ys.len() {
        return Err(StatError::LengthMismatch { a: n, b: ys.len() });
    }
    if n < 2 {
        return Err(StatError::InsufficientData { min: 2, actual: n });
    }
    let mx = compensated_sum(xs.iter().copied()) / n as f64;
    let my = compensated_sum(ys.iter().copied()) / n as f64;
    let mut sxy = 0.0;
    let mut sxx = 0.0;
    let mut syy = 0.0;
    for (&x, &y) in xs.iter().zip(ys) {
        let dx = x - mx;
        let dy = y - my;
        sxy += dx * dy;
        sxx += dx * dx;
        syy += dy * dy;
    }
    let denom = (sxx * syy).sqrt();
    if denom == 0.0 {
        return Err(StatError::InvalidInput(
            "zero variance: at least one input is constant".to_string(),
        ));
    }
    Ok(sxy / denom)
}

// ── Order statistics ───────────────────────────────────────────────────────

/// Minimum value.
pub fn min(xs: &[f64]) -> Result<f64> {
    xs.iter()
        .copied()
        .min_by(|a, b| a.total_cmp(b))
        .ok_or(StatError::EmptyInput)
}

/// Maximum value.
pub fn max(xs: &[f64]) -> Result<f64> {
    xs.iter()
        .copied()
        .max_by(|a, b| a.total_cmp(b))
        .ok_or(StatError::EmptyInput)
}

/// The `k`-th smallest value (1-indexed). Uses partial selection, O(n) average.
pub fn order_statistic(xs: &[f64], k: usize) -> Result<f64> {
    let n = xs.len();
    if n == 0 {
        return Err(StatError::EmptyInput);
    }
    if k == 0 || k > n {
        return Err(StatError::InvalidInput(format!(
            "order statistic index {k} out of range 1..={n}"
        )));
    }
    let mut sorted = xs.to_vec();
    let (_, elt, _) = sorted.select_nth_unstable_by(k - 1, |a, b| a.total_cmp(b));
    Ok(*elt)
}

// ── Quantiles ──────────────────────────────────────────────────────────────

/// Quantile of an already-sorted slice (linear interpolation, R type 7).
pub fn quantile_sorted(sorted: &[f64], q: f64) -> Result<f64> {
    if !(0.0..=1.0).contains(&q) {
        return Err(StatError::InvalidQuantile(q));
    }
    let n = sorted.len();
    if n == 0 {
        return Err(StatError::EmptyInput);
    }
    if n == 1 {
        return Ok(sorted[0]);
    }
    let index = (n as f64 - 1.0) * q;
    let lo = index.floor();
    let hi = index.ceil();
    let frac = index - lo;
    Ok(sorted[lo as usize] * (1.0 - frac) + sorted[hi as usize] * frac)
}

/// Quantile (linear interpolation, R type 7 / NumPy default).
///
/// Copies and sorts internally. For repeated queries, sort once and use
/// [`quantile_sorted`].
pub fn quantile(xs: &[f64], q: f64) -> Result<f64> {
    if xs.is_empty() {
        return Err(StatError::EmptyInput);
    }
    let mut sorted = xs.to_vec();
    sorted.sort_by(|a, b| a.total_cmp(b));
    quantile_sorted(&sorted, q)
}

/// Median (the 0.5 quantile).
pub fn median(xs: &[f64]) -> Result<f64> {
    quantile(xs, 0.5)
}

// ── Ranks ──────────────────────────────────────────────────────────────────

/// Average ranks (ties share the mean rank). Rank 1 is the smallest.
///
/// The foundation for nonparametric methods — Mann-Whitney U (AUC),
/// Spearman correlation, Wilcoxon tests.
pub fn rank(xs: &[f64]) -> Result<Vec<f64>> {
    if xs.is_empty() {
        return Err(StatError::EmptyInput);
    }
    let mut idx: Vec<usize> = (0..xs.len()).collect();
    idx.sort_by(|&a, &b| xs[a].total_cmp(&xs[b]));

    let mut ranks = vec![0.0_f64; xs.len()];
    let mut i = 0;
    while i < idx.len() {
        // Find the end of the tie group.
        let mut j = i + 1;
        while j < idx.len() && xs[idx[j]] == xs[idx[i]] {
            j += 1;
        }
        let avg = ((i + 1) + j) as f64 / 2.0;
        for &k in &idx[i..j] {
            ranks[k] = avg;
        }
        i = j;
    }
    Ok(ranks)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx_eq(a: f64, b: f64, tol: f64) -> bool {
        (a - b).abs() <= tol * b.abs().max(1.0)
    }

    #[test]
    fn mean_sum_basic() {
        let xs = [1.0, 2.0, 3.0, 4.0, 5.0];
        assert_eq!(sum(&xs).unwrap(), 15.0);
        assert_eq!(mean(&xs).unwrap(), 3.0);
    }

    #[test]
    fn empty_is_error() {
        assert!(matches!(mean(&[]), Err(StatError::EmptyInput)));
        assert!(matches!(sum(&[]), Err(StatError::EmptyInput)));
    }

    #[test]
    fn variance_sample_vs_population() {
        let xs = [1.0, 2.0, 3.0, 4.0, 5.0];
        assert!(approx_eq(variance(&xs).unwrap(), 2.5, 1e-12));
        assert!(approx_eq(variance_population(&xs).unwrap(), 2.0, 1e-12));
    }

    #[test]
    fn covariance_and_correlation() {
        let xs = [1.0, 2.0, 3.0, 4.0];
        let ys = [2.0, 4.0, 6.0, 8.0];
        assert!(approx_eq(covariance(&xs, &ys).unwrap(), 10.0 / 3.0, 1e-12));
        assert!(approx_eq(correlation(&xs, &ys).unwrap(), 1.0, 1e-12));
        assert!(approx_eq(
            correlation(&xs, &[-1.0, -2.0, -3.0, -4.0]).unwrap(),
            -1.0,
            1e-12
        ));
    }

    #[test]
    fn correlation_constant_is_error() {
        assert!(matches!(
            correlation(&[1.0, 1.0, 1.0], &[1.0, 2.0, 3.0]),
            Err(StatError::InvalidInput(_))
        ));
    }

    #[test]
    fn min_max_order() {
        let xs = [3.0, 1.0, 4.0, 1.0, 5.0, 9.0, 2.0, 6.0];
        assert_eq!(min(&xs).unwrap(), 1.0);
        assert_eq!(max(&xs).unwrap(), 9.0);
        assert_eq!(order_statistic(&xs, 4).unwrap(), 3.0);
    }

    #[test]
    fn quantile_type7_known_values() {
        let xs = [1.0, 2.0, 3.0, 4.0, 5.0];
        assert_eq!(quantile(&xs, 0.0).unwrap(), 1.0);
        assert_eq!(quantile(&xs, 0.25).unwrap(), 2.0);
        assert_eq!(quantile(&xs, 0.5).unwrap(), 3.0);
        assert_eq!(quantile(&xs, 0.75).unwrap(), 4.0);
        assert_eq!(quantile(&xs, 1.0).unwrap(), 5.0);
    }

    #[test]
    fn quantile_validation() {
        assert!(matches!(
            quantile(&[1.0, 2.0], -0.1),
            Err(StatError::InvalidQuantile(_))
        ));
    }

    #[test]
    fn rank_average_ties() {
        let xs = [1.0, 2.0, 2.0, 3.0];
        assert_eq!(rank(&xs).unwrap(), vec![1.0, 2.5, 2.5, 4.0]);
    }

    #[test]
    fn rank_no_ties() {
        let xs = [3.0, 1.0, 2.0];
        assert_eq!(rank(&xs).unwrap(), vec![3.0, 1.0, 2.0]);
    }
}
