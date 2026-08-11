//! Utility functions ported from `R/utils.R`.
//!
//! These are the building blocks for constructing full V/S matrices when
//! adding SNP effects to the genetic covariance structure.

use faer::Mat;
use serde::{Deserialize, Serialize};

use crate::error::{GenomicSemError, Result};
use crate::linalg;
use crate::near_pd;

// =====================================================================
// Types
// =====================================================================

/// Genomic control correction mode.
///
/// In GenomicSEM, `"standard"` uses `sqrt(I_LD)` to correct SEs,
/// `"conserv"` uses `I_LD` directly, and `"none"` applies no correction.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GcMode {
    Standard,
    Conserv,
    None,
}

impl Default for GcMode {
    fn default() -> Self {
        GcMode::Standard
    }
}

impl std::str::FromStr for GcMode {
    type Err = GenomicSemError;
    fn from_str(s: &str) -> Result<Self> {
        match s {
            "standard" => Ok(GcMode::Standard),
            "conserv" => Ok(GcMode::Conserv),
            "none" => Ok(GcMode::None),
            _ => Err(GenomicSemError::InvalidInput(format!(
                "GC must be 'standard', 'conserv', or 'none', got '{s}'"
            ))),
        }
    }
}

/// The LDSC covariance structure: S (genetic covariance), V (sampling
/// covariance), I (intercepts), N (sample sizes), m (number of SNPs).
///
/// This is the output of [`crate::ldsc::ldsc`] and the input to SEM functions.
#[derive(Clone, Debug)]
pub struct Covstruc {
    /// Sampling covariance matrix V_LD (lower-triangle vech'd).
    pub v: Mat<f64>,
    /// Genetic covariance matrix S_LD.
    pub s: Mat<f64>,
    /// Intercepts matrix I_LD.
    pub i_mat: Mat<f64>,
    /// Sample sizes vector (length = k*(k+1)/2).
    pub n: Mat<f64>,
    /// Number of SNPs M.
    pub m: f64,
    /// Optional: standardized V (from `stand=TRUE` LDSC).
    pub v_stand: Option<Mat<f64>>,
    /// Optional: standardized S (genetic correlation matrix).
    pub s_stand: Option<Mat<f64>>,
}

impl Covstruc {
    /// Number of traits (dimension of S).
    pub fn k(&self) -> usize {
        self.s.nrows()
    }
}

// =====================================================================
// V_SNP, V_full, S_full — ported from utils.R
// =====================================================================

/// `.get_V_SNP` — construct the k×k sampling covariance for the SNP effects
/// on the phenotypes, corrected for LDSC intercepts.
///
/// * `se_snp_row` — SE values for SNP i across k phenotypes.
/// * `i_ld` — the LDSC intercept matrix.
/// * `var_snp_i` — SNP variance `2*MAF*(1-MAF)` for SNP i.
/// * `gc` — genomic control correction mode.
/// * `coords` — `(x, y)` coordinate pairs for non-NA entries in I_LD.
/// * `k` — number of phenotypes.
pub fn get_v_snp(
    se_snp_row: &[f64],
    i_ld: &Mat<f64>,
    var_snp_i: f64,
    gc: GcMode,
    coords: &[(usize, usize)],
    k: usize,
) -> Mat<f64> {
    let mut v_snp = Mat::zeros(k, k);

    for &(x, y) in coords {
        if x != y {
            let val = match gc {
                GcMode::Conserv => {
                    se_snp_row[y] * se_snp_row[x] * i_ld[(x, y)] * i_ld[(x, x)] * i_ld[(y, y)]
                        * var_snp_i * var_snp_i
                }
                GcMode::Standard => {
                    se_snp_row[y] * se_snp_row[x] * i_ld[(x, y)]
                        * i_ld[(x, x)].sqrt()
                        * i_ld[(y, y)].sqrt()
                        * var_snp_i * var_snp_i
                }
                GcMode::None => {
                    se_snp_row[y] * se_snp_row[x] * i_ld[(x, y)] * var_snp_i * var_snp_i
                }
            };
            v_snp[(x, y)] = val;
        } else {
            let val = match gc {
                GcMode::Conserv => {
                    (se_snp_row[x] * i_ld[(x, x)] * var_snp_i).powi(2)
                }
                GcMode::Standard => {
                    (se_snp_row[x] * i_ld[(x, x)].sqrt() * var_snp_i).powi(2)
                }
                GcMode::None => (se_snp_row[x] * var_snp_i).powi(2),
            };
            v_snp[(x, x)] = val;
        }
    }

    v_snp
}

/// `.get_V_full` — construct the full (k+1)×(k+1) sampling covariance matrix
/// that combines V_LD, V_SNP, and the SNP variance SE.
///
/// The full matrix is block-structured:
/// ```text
/// [ varSNPSE2          0               0      ]
/// [    0           V_SNP(k×k)          0      ]
/// [    0               0           V_LD(z×z)  ]
/// ```
/// where z = k*(k+1)/2 and the full dimension is (k+1)*(k+2)/2.
pub fn get_v_full(k: usize, v_ld: &Mat<f64>, var_snp_se2: f64, v_snp: &Mat<f64>) -> Mat<f64> {
    let dim = (k + 1) * (k + 2) / 2;
    let mut v_full = Mat::zeros(dim, dim);

    // SNP variance SE (position 0)
    v_full[(0, 0)] = var_snp_se2;

    // V_SNP block (positions 1..=k)
    for i in 0..k {
        for j in 0..k {
            v_full[(1 + i, 1 + j)] = v_snp[(i, j)];
        }
    }

    // V_LD block (positions k+1..dim-1 in 0-based, which is R's (k+2):nrow)
    let v_ld_start = k + 1;
    for i in 0..v_ld.nrows() {
        for j in 0..v_ld.ncols() {
            v_full[(v_ld_start + i, v_ld_start + j)] = v_ld[(i, j)];
        }
    }

    v_full
}

/// `.get_S_Full` — construct the full observed covariance matrix including
/// the SNP row/column.
///
/// * `n_phenotypes` — k.
/// * `s_ld` — the genetic covariance matrix (k×k).
/// * `var_snp_i` — SNP variance for SNP i.
/// * `beta_snp_row` — effect estimates for SNP i across k phenotypes.
pub fn get_s_full(
    n_phenotypes: usize,
    s_ld: &Mat<f64>,
    var_snp_i: f64,
    beta_snp_row: &[f64],
    snp_label: &str,
    trait_names: &[String],
) -> Mat<f64> {
    let k = n_phenotypes;
    let dim = k + 1;
    let mut s_full = Mat::zeros(dim, dim);

    // S_LD block
    for i in 0..k {
        for j in 0..k {
            s_full[(1 + i, 1 + j)] = s_ld[(i, j)];
        }
    }

    // SNP row/column
    s_full[(0, 0)] = var_snp_i;
    for p in 0..k {
        let val = var_snp_i * beta_snp_row[p];
        s_full[(0, 1 + p)] = val;
        s_full[(1 + p, 0)] = val;
    }

    // Set column/row names (not directly representable in faer, but we
    // store them separately in the caller). Here we just return the matrix.

    s_full
}

/// `.get_Z_pre` — pre-smoothing Z-statistics for the `smooth_check` feature.
pub fn get_z_pre(
    beta_snp_row: &[f64],
    se_snp_row: &[f64],
    i_ld: &Mat<f64>,
    gc: GcMode,
) -> Vec<f64> {
    let k = beta_snp_row.len();
    let diag = linalg::diag_to_vec(i_ld);
    match gc {
        GcMode::Conserv => {
            (0..k)
                .map(|i| beta_snp_row[i] / (se_snp_row[i] * diag[i]))
                .collect()
        }
        GcMode::Standard => {
            (0..k)
                .map(|i| beta_snp_row[i] / (se_snp_row[i] * diag[i].sqrt()))
                .collect()
        }
        GcMode::None => {
            (0..k).map(|i| beta_snp_row[i] / se_snp_row[i]).collect()
        }
    }
}

// =====================================================================
// .rearrange — ported from utils.R line 96
// =====================================================================

/// `.rearrange` — compute the permutation vector that maps the sampling
/// covariance matrix from the user's variable order to lavaan's internal
/// variable order.
///
/// * `k` — number of observed variables.
/// * `lavaan_var_order` — the variable order as returned by lavaan (i.e.,
///   the row/column names of the model-implied covariance matrix from an
///   initial model fit).
/// * `user_var_names` — the user-specified variable names (matching S_LD
///   column names).
///
/// Returns the permutation indices for `vech`-ordered elements.
pub fn rearrange(k: usize, lavaan_var_order: &[String], user_var_names: &[String]) -> Vec<usize> {
    // Build the index matrix: lower triangle including diagonal, column-major
    let z = k * (k + 1) / 2;
    let mut idx_matrix = vec![vec![0usize; k]; k];
    let mut counter = 0usize;
    for col in 0..k {
        for row in col..k {
            idx_matrix[row][col] = counter;
            idx_matrix[col][row] = counter;
            counter += 1;
        }
    }

    // Find permutation: for each pair (i,j) in lavaan order, find the
    // corresponding index in user order
    let mut perm = Vec::with_capacity(z);

    // Build a mapping from lavaan variable names to user variable names
    for col_lavaan in 0..k {
        for row_lavaan in col_lavaan..k {
            let name_row = &lavaan_var_order[row_lavaan];
            let name_col = &lavaan_var_order[col_lavaan];
            // Find positions in user_var_names
            let user_row = user_var_names.iter().position(|n| n == name_row);
            let user_col = user_var_names.iter().position(|n| n == name_col);
            match (user_row, user_col) {
                (Some(ur), Some(uc)) => {
                    perm.push(idx_matrix[ur][uc]);
                }
                _ => {
                    // Fallback: identity
                    perm.push(idx_matrix[row_lavaan][col_lavaan]);
                }
            }
        }
    }

    perm
}

// =====================================================================
// Liability conversion
// =====================================================================

/// Liability-scale conversion factor for case/control traits.
///
/// `conversion.factor = pop.prev^2 * (1 - pop.prev)^2 / (samp.prev * (1 - samp.prev) * dnorm(qnorm(1 - pop.prev))^2)`
pub fn liability_conversion_factor(pop_prev: f64, samp_prev: f64) -> f64 {
    let p = pop_prev;
    let s = samp_prev;
    let z = crate::stats::qnorm(1.0 - p);
    let d = crate::stats::dnorm(z);
    (p * p * (1.0 - p).powi(2)) / (s * (1.0 - s) * d * d)
}

// =====================================================================
// Smooth check
// =====================================================================

/// Smooth an S or V matrix to nearest PD if needed.
/// Returns (smoothed_matrix, was_smoothed, max_abs_diff_before_after).
pub fn smooth_if_needed(mat: &Mat<f64>) -> (Mat<f64>, bool, f64) {
    let min_eig = near_pd::min_eigenvalue(mat);
    if min_eig <= 0.0 {
        let smoothed = near_pd::near_pd(mat);
        let nr = mat.nrows();
        let nc = mat.ncols();
        let mut max_diff = 0.0f64;
        for i in 0..nr {
            for j in 0..nc {
                let d = (smoothed[(i, j)] - mat[(i, j)]).abs();
                if d > max_diff {
                    max_diff = d;
                }
            }
        }
        (smoothed, true, max_diff)
    } else {
        (mat.clone(), false, 0.0)
    }
}

// =====================================================================
// Standardization
// =====================================================================

/// Standardize a covariance matrix to a correlation matrix:
/// `S_Stand = D⁻¹·S·D⁻¹` where `D = diag(sqrt(diag(S)))`.
pub fn standardize(s: &Mat<f64>) -> Mat<f64> {
    let d = linalg::diag_sqrt(s);
    let d_inv = {
        let n = d.nrows();
        let mut m = Mat::zeros(n, n);
        for i in 0..n {
            let val = d[(i, i)];
            m[(i, i)] = if val > 0.0 { 1.0 / val } else { 0.0 };
        }
        m
    };
    &(&d_inv * s) * &d_inv
}

/// Compute the V_stand matrix as GenomicSEM does:
///
/// 1. Standardize S → S_Stand
/// 2. Compute scaleO = vech(S_Stand / S_LD) (element-wise ratio of lower triangles)
/// 3. Dvcov = sqrt(diag(V_LD))
/// 4. Dvcovl = Dvcov * scaleO
/// 5. Vcor = cov2cor(V_LD)
/// 6. V_stand = diag(Dvcovl) · Vcor · diag(Dvcovl)
pub fn compute_v_stand(s_ld: &Mat<f64>, v_ld: &Mat<f64>) -> Mat<f64> {
    let s_stand = standardize(s_ld);
    let z = s_ld.nrows() * (s_ld.nrows() + 1) / 2;

    // scaleO = lowerTriangle(S_Stand / S_LD, diag=T)
    let n = s_ld.nrows();
    let mut scale_o = Vec::with_capacity(z);
    for j in 0..n {
        for i in j..n {
            let ratio = if s_ld[(i, j)].abs() > 1e-30 {
                s_stand[(i, j)] / s_ld[(i, j)]
            } else {
                0.0
            };
            let s = if ratio.is_nan() { 0.0 } else { ratio };
            scale_o.push(s);
        }
    }

    let dvcov = linalg::sqrt_diag(v_ld);
    let dvcovl: Vec<f64> = (0..z).map(|i| dvcov[i] * scale_o[i]).collect();

    let vcor = linalg::cov2cor(v_ld);

    // V_stand = diag(Dvcovl) · Vcor · diag(Dvcovl)
    let mut v_stand = Mat::zeros(z, z);
    for i in 0..z {
        for j in 0..z {
            v_stand[(i, j)] = dvcovl[i] * vcor[(i, j)] * dvcovl[j];
        }
    }

    v_stand
}

/// Build a weight matrix W from V_stand: diagonal of W = diagonal of V_stand,
/// then invert. Used in the DWLS estimation.
pub fn build_w_from_v_stand(v_stand: &Mat<f64>) -> Mat<f64> {
    let z = v_stand.nrows();
    let mut w = Mat::zeros(z, z);
    for i in 0..z {
        let mut val = v_stand[(i, i)];
        if val.abs() < 2e-9 {
            val = 2e-9;
        }
        w[(i, i)] = 1.0 / val;
    }
    w
}

/// Build the reordered weight matrix from V_Reorder.
pub fn build_w_from_v_reorder(v_reorder: &Mat<f64>, toler: f64) -> Mat<f64> {
    let z = v_reorder.nrows();
    let mut w = Mat::zeros(z, z);
    for i in 0..z {
        let mut val = v_reorder[(i, i)];
        if val.abs() < 2e-9 {
            val = 2e-9;
        }
        w[(i, i)] = if toler > 0.0 {
            1.0 / val.max(toler)
        } else {
            1.0 / val
        };
    }
    w
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_v_snp_standard() {
        let se = vec![0.01, 0.02];
        let i_ld = Mat::identity(2, 2);
        let v_snp = get_v_snp(&se, &i_ld, 0.5, GcMode::Standard, &[(0, 0), (0, 1), (1, 1)], 2);
        // Standard GC: diag = (se * sqrt(1) * varSNP)^2
        let expected_00: f64 = (0.01_f64 * 1.0 * 0.5).powi(2);
        assert!((v_snp[(0, 0)] - expected_00).abs() < 1e-12);
    }

    #[test]
    fn test_get_v_full() {
        let k = 2;
        let v_ld = Mat::identity(3, 3);
        let v_snp = Mat::identity(2, 2);
        let v_full = get_v_full(k, &v_ld, 2.5e-7, &v_snp);
        assert_eq!(v_full.nrows(), 6);
        assert!((v_full[(0, 0)] - 2.5e-7).abs() < 1e-15);
        assert!((v_full[(1, 1)] - 1.0).abs() < 1e-15);
    }

    #[test]
    fn test_liability_conversion() {
        let cf = liability_conversion_factor(0.05, 0.5);
        assert!(cf > 0.0);
        // For a 5% prevalence with 50% sampling, conversion should be ~0.82
        assert!(cf > 0.5 && cf < 1.5);
    }

    #[test]
    fn test_standardize() {
        let mut s = Mat::zeros(2, 2);
        s[(0, 0)] = 4.0;
        s[(1, 1)] = 9.0;
        s[(0, 1)] = 2.0;
        s[(1, 0)] = 2.0;
        let cor = standardize(&s);
        assert!((cor[(0, 0)] - 1.0).abs() < 1e-10);
        assert!((cor[(1, 1)] - 1.0).abs() < 1e-10);
        assert!((cor[(0, 1)] - 2.0 / (2.0 * 3.0)).abs() < 1e-10);
    }

    #[test]
    fn test_rearrange_identity() {
        let names = vec!["A".to_string(), "B".to_string(), "C".to_string()];
        let perm = rearrange(3, &names, &names);
        // For identity ordering, perm should be 0,1,2,3,4,5
        assert_eq!(perm, vec![0, 1, 2, 3, 4, 5]);
    }
}
