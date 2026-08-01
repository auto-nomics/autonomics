//! Core test statistics: **SHom** (homogeneous-effects test) and **SHet**
//! (heterogeneous-effects truncated test).
//!
//! Direct 1:1 port of `Non_Trucated_TestScore` and `Trucated_TestScore` from
//! CPASSOC's `FunctionSet.R` (Zhu & Feng, *AJHG* 2015, Eq. 1–3).
//!
//! ## Weight vector
//! Given a vector of per-statistic sample sizes `SampleSize` (length K),
//! ```text
//!   Wi  = SampleSize           (in R: sqrt(n_j), passed in as SampleSize)
//!   sumW = sqrt(Σ Wi²)
//!   W   = Wi / sumW             (normalised weight vector)
//! ```
//! **Note**: In the R reference, `SampleSize` already holds `sqrt(n_j)` values
//! (or the raw sample sizes — the code treats them identically since they're
//! only used as relative weights). We follow the same convention: the caller
//! supplies the weight values, and we normalise.

use crate::linalg::weighted_score;
use faer::Mat;

/// SHom options (all fields mirror the R `Non_Trucated_TestScore` signature).
#[derive(Clone, Copy, Debug)]
pub struct ShomOptions;

/// SHet options (mirrors `Trucated_TestScore` parameters).
#[derive(Clone, Copy, Debug)]
pub struct ShetOptions {
    /// `correct`: `1` ⇒ use absolute statistics with signed weights (default).
    /// Other ⇒ keep original signs (no sign correction on weights).
    pub correct: i32,
    /// `startCutoff` (default 0). Used only when `is_all_possible = false`.
    pub start_cutoff: f64,
    /// `endCutoff` (default 1).
    pub end_cutoff: f64,
    /// `CutoffStep` (default 0.05).
    pub cutoff_step: f64,
    /// `isAllpossible`: `true` ⇒ search all unique `|x|` values as cutoffs.
    pub is_all_possible: bool,
}

impl Default for ShetOptions {
    fn default() -> Self {
        Self {
            correct: 1,
            start_cutoff: 0.0,
            end_cutoff: 1.0,
            cutoff_step: 0.05,
            is_all_possible: true,
        }
    }
}

/// Normalise the sample-size vector into the CPASSOC weight vector `W`.
///
/// `W = SampleSize / sqrt(Σ SampleSize²)`
pub fn weight_vector(sample_size: &[f64]) -> Vec<f64> {
    let sum_sq: f64 = sample_size.iter().map(|w| w * w).sum::<f64>();
    let sum_w = sum_sq.sqrt();
    sample_size.iter().map(|w| w / sum_w).collect()
}

// ─── SHom ───────────────────────────────────────────────────────────────────

/// Compute **SHom** statistics for an M×K matrix of summary statistics.
///
/// Port of `Non_Trucated_TestScore(X, SampleSize, CorrMatrix)`.
///
/// - `x` — M×K matrix (M SNPs × K traits), row-major (`x[m*k..m*k+k]`).
/// - `sample_size` — length-K vector of per-trait sample sizes (weights).
/// - `corr_matrix` — K×K correlation matrix R.
///
/// Returns a length-M vector of SHom statistics. Each follows χ²₁ under the
/// null.
pub fn shom(x: &Mat<f64>, sample_size: &[f64], corr_matrix: &Mat<f64>) -> Vec<f64> {
    let w = weight_vector(sample_size);
    let m = x.nrows();
    (0..m)
        .map(|i| {
            let row: Vec<f64> = (0..x.ncols()).map(|j| x[(i, j)]).collect();
            shom_single(&row, &w, corr_matrix.as_ref())
        })
        .collect()
}

/// SHom for a single SNP (one row vector of K statistics).
///
/// `T = (W · Σ · x)² / (W · Σ · W)`  where `Σ = ginv(R)`.
pub fn shom_single(x: &[f64], w: &[f64], corr_matrix: faer::MatRef<f64>) -> f64 {
    let (num, den) = weighted_score(corr_matrix, w, x);
    let t = num * num / den;
    t
}

// ─── SHet ───────────────────────────────────────────────────────────────────

/// Compute **SHet** statistics for an M×K matrix of summary statistics.
///
/// Port of `Trucated_TestScore(X, SampleSize, CorrMatrix, ...)`.
///
/// Returns a length-M vector of SHet statistics.
pub fn shet(
    x: &Mat<f64>,
    sample_size: &[f64],
    corr_matrix: &Mat<f64>,
    opts: ShetOptions,
) -> Vec<f64> {
    let w = weight_vector(sample_size);
    let m = x.nrows();
    (0..m)
        .map(|i| {
            let row: Vec<f64> = (0..x.ncols()).map(|j| x[(i, j)]).collect();
            shet_single(&row, &w, corr_matrix.as_ref(), opts)
        })
        .collect()
}

/// SHet for a single SNP: the maximum truncated test statistic over all
/// cutoff thresholds.
///
/// Faithful port of the R loop body in `Trucated_TestScore`:
/// ```text
/// for threshold in cutoffs:
///     drop indices where |x| < threshold
///     if all dropped → break
///     submatrix A, subvector x1, subvector W1
///     if correct==1: flip sign of W1 where x1 < 0
///     S(t) = (W1 · ginv(A) · x1)² / (W1 · ginv(A) · W1)
///     track max
/// ```
pub fn shet_single(x: &[f64], w: &[f64], corr_matrix: faer::MatRef<f64>, opts: ShetOptions) -> f64 {
    let n = x.len();
    debug_assert_eq!(w.len(), n);
    debug_assert_eq!(corr_matrix.nrows(), n);
    debug_assert_eq!(corr_matrix.ncols(), n);

    // Build the list of cutoff thresholds.
    let cutoffs: Vec<f64> = if opts.is_all_possible {
        // sort(unique(abs(x)))
        let mut abs_vals: Vec<f64> = x.iter().map(|v| v.abs()).collect();
        // NaN-safe sort: NaN sorts as +∞ so it ends up last (and will cause
        // all elements to be dropped, yielding S(t)=0 for that threshold).
        abs_vals.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Greater));
        abs_vals.dedup();
        abs_vals
    } else {
        // seq(startCutoff, endCutoff, CutoffStep)
        let mut vals = Vec::new();
        let mut v = opts.start_cutoff;
        while v <= opts.end_cutoff + 1e-15 {
            vals.push(v);
            v += opts.cutoff_step;
        }
        vals
    };

    let mut ttt: f64 = -1.0; // R: TTT = -1

    for &threshold in &cutoffs {
        // index = which(abs(x1) < threshold)  → indices to drop
        let drop: Vec<usize> = (0..n).filter(|&i| x[i].abs() < threshold).collect();

        // if (length(index) == N) break
        if drop.len() == n {
            break;
        }

        // Keep = complement of drop
        let keep: Vec<usize> = (0..n).filter(|i| !drop.contains(i)).collect();

        // Extract subvector x1, subweights w1, submatrix A
        let x1: Vec<f64> = keep.iter().map(|&i| x[i]).collect();
        let mut w1: Vec<f64> = keep.iter().map(|&i| w[i]).collect();

        if opts.correct == 1 {
            // index = which(x1 < 0); W1[index] = -W1[index]
            // We need to check x1 (the kept values), not w1.
            for (i, &xi) in x1.iter().enumerate() {
                if xi < 0.0 {
                    w1[i] = -w1[i];
                }
            }
        }

        // Build submatrix A from kept indices
        let kk = keep.len();
        let a_sub = Mat::from_fn(kk, kk, |i, j| corr_matrix[(keep[i], keep[j])]);

        let (num, den) = weighted_score(a_sub.as_ref(), &w1, &x1);
        let t = num * num / den;

        if ttt < t {
            ttt = t;
        }
    }

    ttt
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shom_single_independent() {
        // 2 independent traits, equal weight, identity correlation
        let x = vec![2.0, 2.0];
        let sample_size = vec![100.0, 100.0];
        let w = weight_vector(&sample_size);
        let corr = Mat::identity(2, 2);
        let stat = shom_single(&x, &w, corr.as_ref());
        // With identity R and equal weights: w = [1/√2, 1/√2]
        // Σ = I, num = w·x = (2+2)/√2 = 4/√2 = 2√2
        // den = w·w = (1/2+1/2) = 1
        // stat = (2√2)²/1 = 8
        assert!((stat - 8.0).abs() < 1e-10, "got {stat}");
    }

    #[test]
    fn shet_single_all_positive() {
        // All stats positive → sign correction has no effect
        let x = vec![1.0, 2.0, 3.0];
        let sample_size = vec![100.0, 100.0, 100.0];
        let w = weight_vector(&sample_size);
        let corr = Mat::identity(3, 3);
        let stat = shet_single(&x, &w, corr.as_ref(), ShetOptions::default());
        // With identity R and equal weights, the maximum should be at the
        // largest threshold that keeps only the largest stat (3.0):
        // single-element: (w·x)²/(w·w) = x² (since w cancels)
        // But we also need to check all subthresholds.
        assert!(stat > 0.0);
    }
}
