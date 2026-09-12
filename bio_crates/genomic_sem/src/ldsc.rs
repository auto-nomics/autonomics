//! Multivariate LD Score Regression — port of `R/ldsc.R`.
//!
//! Estimates the genetic covariance matrix `S`, its sampling covariance `V`,
//! and intercepts `I` from munged GWAS summary statistics using multivariate
//! LD Score regression with block jackknife standard errors.

use faer::Mat;
use serde::{Deserialize, Serialize};

use crate::error::{GenomicSemError, Result};
use crate::linalg;
use crate::near_pd;
use crate::stats;
use crate::utils::{Covstruc, smooth_if_needed};

// =====================================================================
// Configuration
// =====================================================================

/// Configuration for multivariate LD Score regression.
#[derive(Clone, Debug)]
pub struct LdscConfig {
    /// Paths to munged summary statistics files (one per trait).
    pub traits: Vec<String>,
    /// Sample prevalence for each trait (NA for continuous).
    pub sample_prev: Vec<Option<f64>>,
    /// Population prevalence for each trait (NA for continuous).
    pub population_prev: Vec<Option<f64>>,
    /// Path to LD score files directory.
    pub ld: String,
    /// Path to weight LD score files directory.
    pub wld: String,
    /// Trait names for output matrices.
    pub trait_names: Option<Vec<String>>,
    /// Whether LD scores and weights are in separate directories.
    pub sep_weights: bool,
    /// Number of chromosomes.
    pub chr: usize,
    /// Number of jackknife blocks.
    pub n_blocks: usize,
    /// Whether to compute standardized (correlation) output.
    pub stand: bool,
    /// Maximum chi-square value for SNP filtering.
    pub chisq_max: Option<f64>,
}

impl Default for LdscConfig {
    fn default() -> Self {
        Self {
            traits: Vec::new(),
            sample_prev: Vec::new(),
            population_prev: Vec::new(),
            ld: String::new(),
            wld: String::new(),
            trait_names: None,
            sep_weights: false,
            chr: 22,
            n_blocks: 200,
            stand: false,
            chisq_max: None,
        }
    }
}

/// LDSC regression output — the `S`, `V`, `I`, `N`, `m` matrices.
#[derive(Clone, Debug)]
pub struct LdscOutput {
    /// Genetic covariance matrix (possibly on liability scale).
    pub s: Mat<f64>,
    /// Sampling covariance matrix.
    pub v: Mat<f64>,
    /// Intercepts matrix.
    pub i_mat: Mat<f64>,
    /// Sample sizes (N.bar per element).
    pub n: Mat<f64>,
    /// Total number of SNPs M.
    pub m: f64,
    /// Standardized genetic correlation matrix (if `stand = true`).
    pub v_stand: Option<Mat<f64>>,
    /// Standardized sampling covariance (if `stand = true`).
    pub s_stand: Option<Mat<f64>>,
    /// Trait names.
    pub trait_names: Vec<String>,
}

impl LdscOutput {
    /// Convert to a [`Covstruc`].
    pub fn to_covstruc(&self) -> Covstruc {
        Covstruc {
            v: self.v.clone(),
            s: self.s.clone(),
            i_mat: self.i_mat.clone(),
            n: self.n.clone(),
            m: self.m,
            v_stand: self.v_stand.clone(),
            s_stand: self.s_stand.clone(),
        }
    }
}

// =====================================================================
// Core LDSC regression (block jackknife)
// =====================================================================

/// Run the weighted least squares regression with block jackknife on a
/// single pair of traits (or a single trait for heritability).
///
/// * `weighted_ld` — n×2 matrix [L2, intercept] × weights
/// * `weighted_chi` — n-vector (chi or ZZ) × weights
/// * `n_blocks` — number of jackknife blocks
/// * `n_snps` — total number of SNPs
///
/// Returns (coefficient, intercept, pseudo_values[:, 0], jackknife_cov)
pub struct JackknifeResult {
    pub reg: Vec<f64>,
    pub intercept: f64,
    pub coef: f64,
    pub reg_tot: f64,
    pub pseudo_values_col0: Vec<f64>,
    pub jackknife_cov: Mat<f64>,
    pub intercept_se: f64,
    pub tot_se: f64,
}

/// Perform the block jackknife regression.
///
/// This is the heart of the LDSC algorithm: weighted least squares with
/// block jackknife to estimate standard errors.
///
/// **Note**: This function applies `weights` uniformly to both the design
/// matrix and the response. For the cross-trait GenomicSEM case where
/// different weights are needed for X vs y, use
/// [`block_jackknife_regression_r`] instead.
pub fn block_jackknife_regression(
    l2: &[f64],
    chi: &[f64],
    weights: &[f64],
    n_blocks: usize,
    n_bar: f64,
    m: f64,
) -> JackknifeResult {
    block_jackknife_regression_r(l2, chi, weights, weights, n_blocks, n_bar, m)
}

/// R-compatible block jackknife regression with **separate weights** for the
/// design matrix (X) and the response (y).
///
/// In GenomicSEM's `ldsc.R`:
/// - For heritability (j==k): `weights_ld == weights_chi` (same weights).
/// - For genetic covariance (j≠k): `weights_ld` = trait-j weights;
///   `weights_chi` = average of trait-j and trait-k weights.
///
/// Also fixes the jackknife covariance: R's `cov(pv)/n.blocks` divides by
/// `(n.blocks - 1) * n.blocks`, while the original Rust version only divided
/// by `n.blocks`.
pub fn block_jackknife_regression_r(
    l2: &[f64],
    chi: &[f64],
    weights_ld: &[f64],
    weights_chi: &[f64],
    n_blocks: usize,
    n_bar: f64,
    m: f64,
) -> JackknifeResult {
    let n_snps = l2.len();
    let n_annot = 1;

    // Build weighted LD (n_snps × 2) and weighted chi (n_snps,)
    // R: weighted.LD <- cbind(L2, intercept) * weights
    //    weighted.chi <- chi * weights_chi   (different weights for gencov!)
    let mut weighted_ld = vec![vec![0.0; n_annot + 1]; n_snps];
    let mut weighted_chi = vec![0.0; n_snps];
    for i in 0..n_snps {
        weighted_ld[i][0] = l2[i] * weights_ld[i];
        weighted_ld[i][1] = 1.0 * weights_ld[i]; // intercept
        weighted_chi[i] = chi[i] * weights_chi[i];
    }

    // Block boundaries — matches R's floor(seq(1, n.snps, length.out=n.blocks+1))
    let select_from = block_boundaries(n_snps, n_blocks, 0);
    let select_to = block_tos(&select_from, n_snps, n_blocks);

    // Per-block XtY and XtX
    let mut xty_blocks = vec![vec![0.0; n_annot + 1]; n_blocks];
    let mut xtx_blocks = vec![vec![vec![0.0; n_annot + 1]; n_annot + 1]; n_blocks];

    for b in 0..n_blocks {
        let from = select_from[b];
        let to = select_to[b];
        for i in from..=to {
            // XtY
            for j in 0..(n_annot + 1) {
                xty_blocks[b][j] += weighted_ld[i][j] * weighted_chi[i];
            }
            // XtX
            for j in 0..(n_annot + 1) {
                for k in 0..(n_annot + 1) {
                    xtx_blocks[b][j][k] += weighted_ld[i][j] * weighted_ld[i][k];
                }
            }
        }
    }

    // Total XtY and XtX
    let mut xty = vec![0.0; n_annot + 1];
    let mut xtx = vec![vec![0.0; n_annot + 1]; n_annot + 1];
    for b in 0..n_blocks {
        for j in 0..(n_annot + 1) {
            xty[j] += xty_blocks[b][j];
        }
        for j in 0..(n_annot + 1) {
            for k in 0..(n_annot + 1) {
                xtx[j][k] += xtx_blocks[b][j][k];
            }
        }
    }

    // Solve xtx · reg = xty
    let reg = solve_small_pub(&xtx, &xty);
    let intercept = reg[n_annot];
    let coef = reg[0] / n_bar;
    let reg_tot = coef * m;

    // Delete-one jackknife
    let mut delete_values = vec![vec![0.0; n_annot + 1]; n_blocks];
    for b in 0..n_blocks {
        let mut xty_del = xty.clone();
        let mut xtx_del = xtx.clone();
        for j in 0..(n_annot + 1) {
            xty_del[j] -= xty_blocks[b][j];
            for k in 0..(n_annot + 1) {
                xtx_del[j][k] -= xtx_blocks[b][j][k];
            }
        }
        delete_values[b] = solve_small_pub(&xtx_del, &xty_del);
    }

    // Pseudo-values: R: pv = n.blocks * reg - (n.blocks-1) * delete
    let nb = n_blocks as f64;
    let mut pseudo_values = vec![vec![0.0; n_annot + 1]; n_blocks];
    for b in 0..n_blocks {
        for j in 0..(n_annot + 1) {
            pseudo_values[b][j] = nb * reg[j] - (nb - 1.0) * delete_values[b][j];
        }
    }

    // Jackknife covariance — R: cov(pseudo.values) / n.blocks
    // R's cov() divides by (n-1), so total denom = n * (n-1)
    let mut jack_cov = vec![vec![0.0; n_annot + 1]; n_annot + 1];
    let mut means = vec![0.0; n_annot + 1];
    for j in 0..(n_annot + 1) {
        let mut s = 0.0;
        for b in 0..n_blocks {
            s += pseudo_values[b][j];
        }
        means[j] = s / nb;
    }
    let denom_cov = (n_blocks as f64) * ((n_blocks - 1) as f64);
    for j in 0..(n_annot + 1) {
        for k in 0..(n_annot + 1) {
            let mut s = 0.0;
            for b in 0..n_blocks {
                s += (pseudo_values[b][j] - means[j]) * (pseudo_values[b][k] - means[k]);
            }
            jack_cov[j][k] = s / denom_cov;
        }
    }

    let intercept_se = jack_cov[n_annot][n_annot].max(0.0).sqrt();

    // Coef covariance and total SE
    let coef_cov = jack_cov[0][0] / (n_bar * n_bar);
    let cat_cov = coef_cov * m * m;
    let tot_se = cat_cov.max(0.0).sqrt();

    // Extract pseudo_values column 0 (for V.hold)
    let pseudo_col0: Vec<f64> = pseudo_values.iter().map(|r| r[0]).collect();

    JackknifeResult {
        reg,
        intercept,
        coef,
        reg_tot,
        pseudo_values_col0: pseudo_col0,
        jackknife_cov: Mat::from_fn(n_annot + 1, n_annot + 1, |i, j| jack_cov[i][j]),
        intercept_se,
        tot_se,
    }
}

/// Block start indices (floor of evenly spaced points).
fn block_boundaries(n_snps: usize, n_blocks: usize, _offset: usize) -> Vec<usize> {
    let mut out = Vec::with_capacity(n_blocks);
    for i in 0..n_blocks {
        let val = ((i as f64) * (n_snps as f64 - 1.0) / (n_blocks as f64)).floor() as usize;
        out.push(val);
    }
    out
}

/// Block end indices: start of next block minus 1, last block gets n_snps-1.
fn block_tos(from: &[usize], n_snps: usize, n_blocks: usize) -> Vec<usize> {
    let mut out = Vec::with_capacity(n_blocks);
    for i in 0..n_blocks {
        if i + 1 < n_blocks {
            out.push(from[i + 1].saturating_sub(1));
        } else {
            out.push(n_snps - 1);
        }
    }
    out
}

/// Solve a small linear system A·x = b using Gaussian elimination with partial pivoting.
pub fn solve_small_pub(a: &[Vec<f64>], b: &[f64]) -> Vec<f64> {
    let n = b.len();
    let mut aug = vec![vec![0.0; n + 1]; n];
    for i in 0..n {
        for j in 0..n {
            aug[i][j] = a[i][j];
        }
        aug[i][n] = b[i];
    }

    // Forward elimination with partial pivoting
    for col in 0..n {
        // Find pivot
        let mut max_row = col;
        let mut max_val = aug[col][col].abs();
        for row in (col + 1)..n {
            if aug[row][col].abs() > max_val {
                max_val = aug[row][col].abs();
                max_row = row;
            }
        }
        if max_val < 1e-15 {
            continue; // Singular column, skip
        }
        aug.swap(col, max_row);

        // Eliminate
        for row in (col + 1)..n {
            let factor = aug[row][col] / aug[col][col];
            for j in col..=n {
                aug[row][j] -= factor * aug[col][j];
            }
        }
    }

    // Back substitution
    let mut x = vec![0.0; n];
    for i in (0..n).rev() {
        let mut sum = aug[i][n];
        for j in (i + 1)..n {
            sum -= aug[i][j] * x[j];
        }
        x[i] = if aug[i][i].abs() > 1e-15 {
            sum / aug[i][i]
        } else {
            0.0
        };
    }
    x
}

/// Compute liability scaling vector.
pub fn compute_liab_s(
    sample_prev: &[Option<f64>],
    population_prev: &[Option<f64>],
    n_traits: usize,
) -> Vec<f64> {
    let mut liab_s = vec![1.0; n_traits];
    for j in 0..n_traits {
        // Guard: slices may be shorter than n_traits (e.g. continuous traits
        // with no prevalence info). Only compute conversion if both are present.
        if j >= sample_prev.len() || j >= population_prev.len() {
            continue;
        }
        if let (Some(sp), Some(pp)) = (sample_prev[j], population_prev[j]) {
            if sp.is_finite() && pp.is_finite() && sp > 0.0 && pp > 0.0 {
                liab_s[j] = crate::utils::liability_conversion_factor(pp, sp);
            }
        }
    }
    liab_s
}

/// Scale the S and V matrices to liability scale.
///
/// * `cov` — the genetic covariance matrix (observed scale)
/// * `v_out` — the raw sampling covariance (from jackknife)
/// * `liab_s` — liability conversion factors per trait
/// * `n_vec` — sample sizes per element
/// * `n_blocks` — number of jackknife blocks
/// * `m` — total number of SNPs
///
/// Returns (S, V) on liability scale.
pub fn scale_to_liability(
    cov: &Mat<f64>,
    v_raw: &Mat<f64>,
    liab_s: &[f64],
    _n_vec: &Mat<f64>,
    _n_blocks: usize,
    _m: f64,
) -> (Mat<f64>, Mat<f64>) {
    let k = cov.nrows();

    // S = cov * tcrossprod(sqrt(liab_s))  — ELEMENT-WISE (R's `*` operator)
    // NOT matrix multiplication! R uses `cov * ratio` which is element-wise.
    // faer's `*` on Mat is matrix multiplication, so we must use explicit
    // element-wise multiplication to match R semantics.
    let mut ratio = Mat::zeros(k, k);
    for i in 0..k {
        for j in 0..k {
            ratio[(i, j)] = (liab_s[i] * liab_s[j]).sqrt();
        }
    }
    let mut s = Mat::zeros(k, k);
    for i in 0..k {
        for j in 0..k {
            s[(i, j)] = cov[(i, j)] * ratio[(i, j)];
        }
    }

    // scaleO = lowerTriangle(ratio, diag=TRUE)
    let scale_o = linalg::vech(&ratio);

    // V = v_raw * tcrossprod(scaleO)
    let mut v = Mat::zeros(v_raw.nrows(), v_raw.ncols());
    for i in 0..v_raw.nrows() {
        for j in 0..v_raw.ncols() {
            v[(i, j)] = v_raw[(i, j)] * scale_o[i] * scale_o[j];
        }
    }

    (s, v)
}

/// Assemble the full LDSC output from per-trait/pair regression results.
///
/// Given the S (covariance), V (sampling covariance from pseudo-values),
/// intercepts, and N vector, this applies liability scaling and returns
/// the standard GenomicSEM output.
pub fn assemble_output(
    cov: Mat<f64>,
    v_hold: &Mat<f64>,
    n_vec: Mat<f64>,
    intercepts: Mat<f64>,
    liab_s: &[f64],
    m: f64,
    n_blocks: usize,
    trait_names: Vec<String>,
    stand: bool,
) -> LdscOutput {
    // Scale V: v_out = cov(V.hold) / crossprod(N.vec * (sqrt(n.blocks) / m))
    let v_raw = linalg::row_cov(v_hold);

    let k = cov.nrows();
    let z = k * (k + 1) / 2;

    // Denominator: crossprod(N.vec * (sqrt(n.blocks) / m))
    // crossprod = N' * N for a vector → sum of squared elements
    let scale_factor = (n_blocks as f64).sqrt() / m;
    let mut v_out = Mat::zeros(z, z);
    for i in 0..z {
        for j in 0..z {
            let denom = n_vec[(0, i)] * n_vec[(0, j)] * scale_factor * scale_factor;
            v_out[(i, j)] = if denom > 0.0 {
                v_raw[(i, j)] / denom
            } else {
                v_raw[(i, j)]
            };
        }
    }

    // Scale S and V to liability
    let (s, v) = scale_to_liability(&cov, &v_out, liab_s, &n_vec, n_blocks, m);

    // Compute standardized output if requested
    let (s_stand_opt, v_stand_opt) = if stand {
        // Check for positive heritability
        let all_positive = (0..k).all(|i| s[(i, i)] > 0.0);
        if all_positive {
            let s_stand = crate::utils::standardize(&s);
            let v_stand = crate::utils::compute_v_stand(&s, &v);
            (Some(s_stand), Some(v_stand))
        } else {
            (None, None)
        }
    } else {
        (None, None)
    };

    LdscOutput {
        s,
        v,
        i_mat: intercepts,
        n: n_vec,
        m,
        v_stand: v_stand_opt,
        s_stand: s_stand_opt,
        trait_names,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_solve_small_2x2() {
        let a = vec![vec![2.0, 1.0], vec![1.0, 3.0]];
        let b = vec![3.0, 5.0];
        let x = solve_small_pub(&a, &b);
        // Solution: x = [0.8, 1.4]
        assert!((x[0] - 0.8).abs() < 1e-10);
        assert!((x[1] - 1.4).abs() < 1e-10);
    }

    #[test]
    fn test_block_boundaries() {
        let from = block_boundaries(1000, 4, 0);
        let to = block_tos(&from, 1000, 4);
        assert_eq!(from.len(), 4);
        assert_eq!(to.len(), 4);
        assert_eq!(to[3], 999);
    }

    #[test]
    fn test_jackknife_regression_basic() {
        // Simple synthetic data: y = 2*x + intercept
        let n = 500;
        let l2: Vec<f64> = (0..n).map(|i| (i as f64) / 100.0).collect();
        let chi: Vec<f64> = l2.iter().map(|x| 2.0 * x + 1.0).collect();
        let weights: Vec<f64> = vec![1.0 / n as f64; n];

        let result = block_jackknife_regression(&l2, &chi, &weights, 10, n as f64, 1000.0);
        assert!(result.intercept > 0.5 && result.intercept < 1.5);
    }
}
