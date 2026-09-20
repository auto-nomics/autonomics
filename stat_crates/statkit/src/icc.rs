//! Intraclass correlation coefficient, two-way random effects, absolute
//! agreement, single measurement — ICC(2,1) (Shrout & Fleiss 1979 type 2,
//! McGraw & Wong 1996 ICC(A,1)).
//!
//! This is the agreement statistic the radiomics SAP mandates for
//! double-read reproducibility (segmentation-derived features, blinded
//! histology scores): absolute agreement, not consistency, so systematic
//! rater bias — one reader consistently doubling a feature, say — lowers
//! the coefficient even though Pearson r would stay at 1.
//!
//! Estimates and the F-based (1−α) confidence interval follow the
//! McGraw & Wong formulation as implemented in `irr`/`psych`:
//!
//! ```text
//! ICC(2,1) = (MSR − MSE) / (MSR + (k−1)·MSE + (k/n)·(MSC − MSE))
//! ```
//!
//! with `MSR` the between-subject mean square, `MSC` the between-rater mean
//! square and `MSE` the residual mean square. The CI substitutes
//! `F = MSR/MSE` by its (1−α/2) F-bounds with `ν₁ = n−1`, `ν₂ = (n−1)(k−1)`
//! into the same expression (with `MSC/MSE` held fixed).

use crate::error::{Result, StatError};
use statrs::distribution::{ContinuousCDF, FisherSnedecor};

/// Result of an ICC(2,1) computation.
#[derive(Debug, Clone)]
pub struct IccResult {
    /// Point estimate ICC(2,1), in `[−1, 1]`.
    pub icc: f64,
    /// Two-sided confidence interval, clamped to `[−1, 1]`.
    pub ci: (f64, f64),
    /// Confidence level actually used (`1 − α`, default 0.95).
    pub level: f64,
    /// Number of subjects (rows).
    pub n_subjects: usize,
    /// Number of raters (columns).
    pub n_raters: usize,
    /// Between-subject mean square `MSR`.
    pub ms_subjects: f64,
    /// Between-rater mean square `MSC`.
    pub ms_raters: f64,
    /// Residual mean square `MSE`.
    pub ms_error: f64,
}

/// Compute ICC(2,1) from one row of ratings per subject at the 95% level.
pub fn icc_2_1(rows: &[&[f64]]) -> Result<IccResult> {
    icc_2_1_level(rows, 0.05)
}

/// Compute ICC(2,1) at a caller-chosen two-sided significance level.
///
/// * `rows` — one slice per subject, each holding that subject's ratings
///   from every rater (a balanced two-way layout — double-read
///   reproducibility subsets are);
/// * `alpha` — two-sided level for the CI (`0.05` → 95%).
pub fn icc_2_1_level(rows: &[&[f64]], alpha: f64) -> Result<IccResult> {
    let n = rows.len();
    if n == 0 {
        return Err(StatError::InvalidInput(
            "ratings must be non-empty".to_string(),
        ));
    }
    let k = rows[0].len();
    if rows.iter().any(|r| r.len() != k) {
        return Err(StatError::InvalidInput(
            "every subject must have the same number of ratings".to_string(),
        ));
    }
    if !(0.0..1.0).contains(&alpha) {
        return Err(StatError::InvalidInput(format!(
            "alpha must lie in (0, 1), got {alpha}"
        )));
    }
    if k < 2 {
        return Err(StatError::InvalidInput(
            "ICC needs at least two raters".to_string(),
        ));
    }
    if n < 2 {
        return Err(StatError::InvalidInput(
            "ICC needs at least two subjects".to_string(),
        ));
    }

    // Row (subject) means, column (rater) means, grand mean.
    let mut row_mean = vec![0.0_f64; n];
    let mut col_mean = vec![0.0_f64; k];
    let mut grand = 0.0_f64;
    for (i, row) in rows.iter().enumerate() {
        for (j, &x) in row.iter().enumerate() {
            if !x.is_finite() {
                return Err(StatError::InvalidInput(format!(
                    "rating at subject {i}, rater {j} is not finite"
                )));
            }
            row_mean[i] += x;
            col_mean[j] += x;
            grand += x;
        }
        row_mean[i] /= k as f64;
    }
    for j in 0..k {
        col_mean[j] /= n as f64;
    }
    grand /= (n * k) as f64;

    let ss_subjects: f64 = (0..n)
        .map(|i| k as f64 * (row_mean[i] - grand).powi(2))
        .sum();
    let ss_raters: f64 = (0..k)
        .map(|j| n as f64 * (col_mean[j] - grand).powi(2))
        .sum();
    let ss_total: f64 = rows
        .iter()
        .flat_map(|r| r.iter().map(|&x| (x - grand).powi(2)))
        .sum();
    let ss_error = ss_total - ss_subjects - ss_raters;

    let msr = ss_subjects / (n - 1) as f64;
    let msc = ss_raters / (k - 1) as f64;
    let mse = ss_error / ((n - 1) * (k - 1)) as f64;

    let denom = msr + (k - 1) as f64 * mse + (k as f64 / n as f64) * (msc - mse);
    if denom == 0.0 {
        return Err(StatError::InvalidInput(
            "ratings are constant: ICC is undefined".to_string(),
        ));
    }
    let icc = (msr - mse) / denom;
    let icc = icc.clamp(-1.0, 1.0);

    // F-based confidence bounds.
    let (lo, hi) = if mse <= 0.0 {
        // Zero residual variance: the estimate sits on the parameter
        // boundary and the F substitution degenerates. Report the point
        // estimate as the lower bound and +1 as the upper, and let the
        // caller see `ms_error == 0` for what happened.
        (icc, 1.0)
    } else {
        let nu1 = (n - 1) as f64;
        let nu2 = ((n - 1) * (k - 1)) as f64;
        let f_stat = msr / mse;
        let dist_12 = FisherSnedecor::new(nu1, nu2)
            .map_err(|e| StatError::Numerical(format!("F({nu1},{nu2}): {e}")))?;
        let dist_21 = FisherSnedecor::new(nu2, nu1)
            .map_err(|e| StatError::Numerical(format!("F({nu2},{nu1}): {e}")))?;
        let q = dist_12.inverse_cdf(1.0 - alpha / 2.0);
        let q_inv = dist_21.inverse_cdf(1.0 - alpha / 2.0);
        let f_lo = f_stat / q;
        let f_hi = f_stat * q_inv;
        let scale = (k as f64 / n as f64) * (msc / mse - 1.0);
        let bound = |f: f64| ((f - 1.0) / (f + (k - 1) as f64 + scale)).clamp(-1.0, 1.0);
        (bound(f_lo), bound(f_hi))
    };

    Ok(IccResult {
        icc,
        ci: (lo, hi),
        level: 1.0 - alpha,
        n_subjects: n,
        n_raters: k,
        ms_subjects: msr,
        ms_raters: msc,
        ms_error: mse,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn systematic_rater_bias_lowers_icc_despite_perfect_r() {
        // Rater 2 exactly doubles rater 1: Pearson r = 1, but absolute
        // agreement is broken. Hand-computed MS: MSR = 7.5, MSC = 12.5,
        // MSE = 5/6, ICC(2,1) = 8/17 ≈ 0.4706.
        let rows: [&[f64]; 4] = [&[1.0, 2.0], &[2.0, 4.0], &[3.0, 6.0], &[4.0, 8.0]];
        let r = icc_2_1(&rows).unwrap();
        assert!((r.icc - 8.0 / 17.0).abs() < 1e-12);
        assert!((r.ms_subjects - 7.5).abs() < 1e-12);
        assert!((r.ms_raters - 12.5).abs() < 1e-12);
        assert!((r.ms_error - 5.0 / 6.0).abs() < 1e-12);
        assert!(r.ci.0 <= r.icc && r.icc <= r.ci.1);
    }

    #[test]
    fn identical_raters_give_icc_one() {
        let rows: [&[f64]; 4] = [&[1.0, 1.0], &[2.0, 2.0], &[3.0, 3.0], &[4.0, 4.0]];
        let r = icc_2_1(&rows).unwrap();
        assert!((r.icc - 1.0).abs() < 1e-12);
        assert!(r.ms_error.abs() < 1e-12);
        assert!(r.ms_raters.abs() < 1e-12);
    }

    #[test]
    fn noisy_raters_give_moderate_icc() {
        // Hand-computable: MSR = 8.5, MSC = 0, MSE = 2/3 → ICC = 47/53.
        let rows: [&[f64]; 4] = [&[9.0, 10.0], &[12.0, 11.0], &[15.0, 14.0], &[11.0, 12.0]];
        let r = icc_2_1(&rows).unwrap();
        assert!((r.icc - 47.0 / 53.0).abs() < 1e-12);
        assert!(r.ci.0 < r.icc && r.icc < r.ci.1);
    }

    #[test]
    fn wider_ci_at_higher_level() {
        let rows: [&[f64]; 4] = [&[9.0, 10.0], &[12.0, 11.0], &[15.0, 14.0], &[11.0, 12.0]];
        let a = icc_2_1_level(&rows, 0.05).unwrap();
        let b = icc_2_1_level(&rows, 0.20).unwrap();
        assert!(b.ci.0 >= a.ci.0 && b.ci.1 <= a.ci.1);
    }

    #[test]
    fn constant_ratings_rejected() {
        let rows: [&[f64]; 4] = [&[5.0, 5.0]; 4];
        assert!(icc_2_1(&rows).is_err());
    }
}
