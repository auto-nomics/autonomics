//! LD reference panel — the eigen-decomposed LD correlation matrix for one
//! region.
//!
//! Two construction paths:
//! - **Production** ([`ld_ref_from_plink`]): build `(lam, V, LDsc)` from a PLINK
//!   `.bed` by computing the SNP × SNP LD correlation matrix `R` and
//!   eigendecomposing it — reusing [`lava::plink`] for genotype loading and
//!   [`lava::decompose::sym_eigen`] for the symmetric eigendecomposition. This
//!   mirrors `build_ld_ref/eigen.R` (which eigendecomposes the banded PLINK
//!   `--r` matrix via RSpectra); conceptually identical, just without the banded
//!   optimisation.
//! - **Tests**: the portable-array loading lives in `tests/cross_validation.rs`
//!   (reads `gen_golden.R`'s TSV exports directly).
//!
//! `lam` = eigenvalues of `R` (descending), `V` = eigenvectors (columns),
//! `LDsc` = per-SNP LD scores `rowSums(R∘R)` (used for the WLS starting values).

use std::path::Path;

use faer::{Mat, MatRef};

use crate::error::{HdlError, Result};

/// Per-region eigen LD reference.
#[derive(Debug, Clone)]
pub struct LdReference {
    /// Eigenvalues of the LD correlation matrix, **descending** (R `eigen`
    /// convention; matches [`lava::decompose::sym_eigen`]).
    pub lam: Vec<f64>,
    /// Eigenvectors as columns (`n_snps × n_snps`).
    pub v: Mat<f64>,
    /// Per-SNP LD scores `rowSums(R²)`.
    pub ldsc: Vec<f64>,
    /// Reference SNP ids (lower-cased, matching lava `.bim`).
    pub snps: Vec<String>,
    /// Reference A2 alleles (`.bim` column 6).
    pub a2_ref: Vec<String>,
}

/// Number of leading eigen-components retaining `cut` cumulative variance.
///
/// Faithful port of `HDL.L.R::eigen_select_num.fun` (lines 533-540): if the
/// last component still does not reach `cut`, keep all; otherwise keep up to
/// (and including) the first component whose cumulative share **exceeds** `cut`.
pub fn eigen_select_num(lam: &[f64], cut: f64) -> usize {
    let total: f64 = lam.iter().sum();
    if total <= 0.0 {
        return lam.len();
    }
    let mut cum = 0.0;
    for (i, &l) in lam.iter().enumerate() {
        cum += l;
        if cum / total > cut {
            return i + 1;
        }
    }
    lam.len()
}

/// R's `scale()` (center + divide by sample sd, n-1 denominator), applied
/// column-wise. Zero-variance columns become all-zero (avoiding NaN), matching
/// R `scale()` → `NA` then imputed to 0 as in `gen_golden.R`.
fn scale_columns(x: &Mat<f64>) -> Mat<f64> {
    let m = x.nrows();
    let n = x.ncols();
    let mut out = Mat::zeros(m, n);
    for j in 0..n {
        let mut mean = 0.0;
        for i in 0..m {
            mean += x[(i, j)];
        }
        mean /= m as f64;
        let mut ss = 0.0;
        for i in 0..m {
            let c = x[(i, j)] - mean;
            ss += c * c;
        }
        let sd = (ss / (m - 1) as f64).sqrt();
        if sd < 1e-300 {
            // zero-variance column → leave as zeros (R scale → NaN → 0)
            continue;
        }
        for i in 0..m {
            out[(i, j)] = (x[(i, j)] - mean) / sd;
        }
    }
    out
}

/// Build an [`LdReference`] from a PLINK `.bed/.bim/.fam` prefix, restricted to
/// `keep_snps` (lower-cased ids). Genotypes are loaded via [`lava::plink`], the
/// LD correlation matrix `R = scale(X)ᵀ·scale(X)/(n-1)` is formed, and
/// [`lava::decompose::sym_eigen`] produces `(lam, V)`.
pub fn ld_ref_from_plink(prefix: &Path, keep_snps: &[String]) -> Result<LdReference> {
    let refr = lava::plink::load_reference(prefix)?;
    let n_indiv = refr.sample_size;

    // Resolve the indices (into the merged .bim) of the requested SNPs, in the
    // order requested.
    let keep_lc: Vec<String> = keep_snps.iter().map(|s| s.to_lowercase()).collect();
    let mut bim_idx: Vec<usize> = Vec::with_capacity(keep_lc.len());
    let mut a2_ref: Vec<String> = Vec::with_capacity(keep_lc.len());
    for s in &keep_lc {
        let pos = refr.snp_info.snp.iter().position(|x| x == s);
        match pos {
            Some(p) => {
                bim_idx.push(p);
                a2_ref.push(refr.snp_info.a2[p].clone());
            }
            None => {
                return Err(HdlError::Reference(format!(
                    "SNP {s} not found in reference .bim"
                )));
            }
        }
    }

    // `load_plink` requires ascending-unique indices; sort and remember the
    // permutation so we can restore the requested column order afterwards.
    let n_keep = bim_idx.len();
    let mut perm: Vec<usize> = (0..n_keep).collect();
    perm.sort_by_key(|&k| bim_idx[k]);
    let sorted_idx: Vec<usize> = perm.iter().map(|&k| bim_idx[k]).collect();
    // position of each requested SNP within the sorted column layout:
    let mut col_of_req = vec![0usize; n_keep];
    for (sorted_col, &req_pos) in perm.iter().enumerate() {
        col_of_req[req_pos] = sorted_col;
    }

    let bed = prefix.with_extension("bed");
    // Permissive filter: keep every requested SNP (R computed LD from all).
    let filt = lava::plink::PlinkFilter {
        maf: 0.0,
        mac: 0.0,
        missing: 1.0,
    };
    let pld = lava::plink::load_plink(&bed, n_indiv, &sorted_idx, filt, false)?;
    let geno = pld.genotypes;

    // Remap columns into the REQUESTED order.
    let mut x = Mat::zeros(n_indiv, n_keep);
    let mut snps_out = Vec::with_capacity(n_keep);
    let mut a2_out = Vec::with_capacity(n_keep);
    for req_j in 0..n_keep {
        let col = col_of_req[req_j];
        for i in 0..n_indiv {
            x[(i, req_j)] = geno[(i, col)];
        }
        snps_out.push(keep_lc[req_j].clone());
        a2_out.push(a2_ref[req_j].clone());
    }

    // ---- LD correlation matrix R = scale(X)ᵀ·scale(X)/(n-1) ----
    let xstd = scale_columns(&x);
    let n = xstd.ncols();
    let mut r = Mat::zeros(n, n);
    for i in 0..n {
        for j in 0..n {
            let mut s = 0.0;
            for k in 0..n_indiv {
                s += xstd[(k, i)] * xstd[(k, j)];
            }
            r[(i, j)] = s / (n_indiv - 1) as f64;
        }
    }
    let ldsc: Vec<f64> = (0..n)
        .map(|i| (0..n).map(|j| r[(i, j)] * r[(i, j)]).sum())
        .collect();
    let (lam, v) = lava::decompose::sym_eigen(r.as_ref()).map_err(|e| {
        HdlError::Reference(format!("eigendecomposition failed: {e}"))
    })?;

    Ok(LdReference {
        lam,
        v,
        ldsc,
        snps: snps_out,
        a2_ref: a2_out,
    })
}

/// Build an [`LdReference`] from **all** SNPs in the PLINK `.bim`.
///
/// Convenience for the DAG node, which analyses one region's reference panel
/// in its entirety.
pub fn ld_ref_from_plink_all(prefix: &Path) -> Result<LdReference> {
    let refr = lava::plink::load_reference(prefix)?;
    let snps: Vec<String> = refr.snp_info.snp.clone();
    ld_ref_from_plink(prefix, &snps)
}

/// Symmetrise helper exposed for tests.
#[cfg(test)]
pub(crate) fn _rr_sym(r: MatRef<f64>) -> Mat<f64> {
    let n = r.nrows();
    Mat::from_fn(n, n, |i, j| 0.5 * (r[(i, j)] + r[(j, i)]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eigen_select_num_basic() {
        // lam = [4, 3, 2, 1], total = 10; cumfrac = [.4, .7, .9, 1.0]
        let lam = vec![4.0, 3.0, 2.0, 1.0];
        assert_eq!(eigen_select_num(&lam, 0.39), 1); // 0.4 > 0.39
        assert_eq!(eigen_select_num(&lam, 0.69), 2);
        assert_eq!(eigen_select_num(&lam, 0.99), 4); // 0.9 < 0.99, 1.0 > 0.99
        assert_eq!(eigen_select_num(&lam, 0.999), 4); // never reaches → keep all
    }

    #[test]
    fn scale_matches_r() {
        // column [1,3,5]: mean 3, sd sqrt(((4+0+4)/2))=2 → [-1, 0, 1]
        let x = Mat::from_fn(3, 1, |i, _| [1.0, 3.0, 5.0][i]);
        let s = scale_columns(&x);
        for (i, &e) in [-1.0_f64, 0.0, 1.0].iter().enumerate() {
            assert!((s[(i, 0)] - e).abs() < 1e-12, "got {}", s[(i, 0)]);
        }
    }
}
