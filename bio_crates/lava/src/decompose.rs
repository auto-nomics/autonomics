//! LD decomposition — faithful port of `decompose.ld` (`R/input_processing.R`).
//!
//! Produces the PC-projection matrix `R` (n_snps × K) from either:
//!  - **PLINK genotypes**: `X = scale(scale(data))` (double z-score, NA→0),
//!    `λᵢ = dᵢ²/(N_ref−1)`, `Q = V` from the SVD; or
//!  - **a precomputed LD (correlation) matrix**: `λ, Q` from its symmetric
//!    eigendecomposition.
//!
//! Then PC-pruning retains `K = min{ i : cumsum(λ)/sum ≥ prune_thresh }` PCs and
//! returns `R = Q[:,1:K] · diag(1/√λ[1:K])`. The recursive block path for large
//! loci (>`max_block_size` SNPs) is also implemented.

use faer::{Mat, MatRef};

use crate::error::{LavaError, Result};

/// R's `scale()` (default: center + divide by sample sd, n-1 denominator),
/// applied column-wise. NA entries propagate (a column with any NA becomes all
/// NA), matching R `scale()` semantics.
fn scale_columns(x: &Mat<f64>) -> Mat<f64> {
    let m = x.nrows();
    let n = x.ncols();
    let mut out = Mat::zeros(m, n);
    for j in 0..n {
        // mean (na.rm = FALSE -> NA if any NA)
        let mut mean = 0.0;
        let mut any_na = false;
        for i in 0..m {
            let v = x[(i, j)];
            if v.is_nan() {
                any_na = true;
                break;
            }
            mean += v;
        }
        if any_na {
            for i in 0..m {
                out[(i, j)] = f64::NAN;
            }
            continue;
        }
        mean /= m as f64;
        // centered rms / (n-1)
        let mut ss = 0.0;
        for i in 0..m {
            let c = x[(i, j)] - mean;
            ss += c * c;
        }
        let sd = (ss / (m - 1) as f64).sqrt();
        for i in 0..m {
            out[(i, j)] = (x[(i, j)] - mean) / sd;
        }
    }
    out
}

/// Given eigenvalues (descending) and eigenvectors (columns), pick K by the
/// cumulative-variance threshold and return `R = Q[:,1:K] · diag(1/√λ[1:K])`.
fn build_r_from_eigen(lambda: &[f64], q: MatRef<f64>, prune_thresh: f64) -> (Mat<f64>, usize) {
    let total = lambda.iter().sum::<f64>();
    let mut cum = 0.0;
    let mut k = lambda.len();
    for (i, &l) in lambda.iter().enumerate() {
        cum += l;
        if cum / total * 100.0 >= prune_thresh {
            k = i + 1;
            break;
        }
    }
    let n = q.nrows();
    let mut r = Mat::zeros(n, k);
    for col in 0..k {
        let scl = 1.0 / lambda[col].sqrt();
        for row in 0..n {
            r[(row, col)] = q[(row, col)] * scl;
        }
    }
    (r, k)
}

/// Symmetric eigendecomposition; returns (eigenvalues_desc, eigenvectors).
/// Eigenvalues are sorted in **descending** order (R `eigen` returns
/// descending) with eigenvectors permuted accordingly.
fn sym_eigen(a: MatRef<f64>) -> Result<(Vec<f64>, Mat<f64>)> {
    let n = a.nrows();
    // faer self-adjoint eigendecomposition (high-level API, Result-returning).
    let e = a.self_adjoint_eigen(faer::Side::Lower).map_err(|e| LavaError::Numeric(format!("eigen failed: {e:?}")))?;
    let s = e.S(); // DiagRef
    let u = e.U(); // MatRef
    // faer returns eigenvalues in nondecreasing order; R eigen returns
    // nonincreasing. Sort descending.
    let sv = s.column_vector();
    let mut idx: Vec<usize> = (0..n).collect();
    idx.sort_by(|&i, &j| sv[j].partial_cmp(&sv[i]).unwrap());
    let lambda: Vec<f64> = idx.iter().map(|&i| sv[i]).collect();
    let q = Mat::from_fn(n, n, |row, col| u[(row, idx[col])]);
    Ok((lambda, q))
}

/// Decompose a precomputed LD (correlation) matrix. Faithful port of
/// `decompose.ld` ld-mode (with the recursive block path).
pub fn decompose_ld_matrix(ld: MatRef<f64>, prune_thresh: f64, max_block_size: usize) -> Result<Mat<f64>> {
    let n = ld.ncols();
    if n > max_block_size {
        // block path
        let no_blocks = (n + max_block_size - 1) / max_block_size;
        let mut block_id = vec![0usize; n];
        for (i, b) in block_id.iter_mut().enumerate() {
            *b = i % no_blocks;
        }
        // R uses sort(rep(1:no.blocks, length.out=no.snps)) — round-robin assignment
        // Build R.base column-wise per block.
        let mut r_base_cols: Vec<Mat<f64>> = Vec::new();
        for b in 0..no_blocks {
            let curr: Vec<usize> = (0..n).filter(|&i| block_id[i] == b).collect();
            if curr.is_empty() {
                continue;
            }
            let m = curr.len();
            let mut sub = Mat::zeros(m, m);
            for (a, &ia) in curr.iter().enumerate() {
                for (c, &ic) in curr.iter().enumerate() {
                    sub[(a, c)] = ld[(ia, ic)];
                }
            }
            let r_curr = decompose_ld_matrix(sub.as_ref(), prune_thresh, usize::MAX)?;
            // embed into full n × ncol matrix
            let ncols = r_curr.ncols();
            let mut add = Mat::zeros(n, ncols);
            for (a, &ia) in curr.iter().enumerate() {
                for c in 0..ncols {
                    add[(ia, c)] = r_curr[(a, c)];
                }
            }
            r_base_cols.push(add);
        }
        // concatenate
        let total_cols: usize = r_base_cols.iter().map(|m| m.ncols()).sum();
        let mut r_base = Mat::zeros(n, total_cols);
        let mut off = 0;
        for m in &r_base_cols {
            for c in 0..m.ncols() {
                for row in 0..n {
                    r_base[(row, off + c)] = m[(row, c)];
                }
            }
            off += m.ncols();
        }
        // M = t(R_base) %*% ld %*% R_base
        let rt = r_base.transpose();
        let tmp = &rt * &ld;
        let m_mat = &tmp * &r_base;
        let r_block = decompose_ld_matrix(m_mat.as_ref(), prune_thresh, 2 * max_block_size)?;
        let res = &r_base * &r_block;
        Ok(res)
    } else {
        let (lambda, q) = sym_eigen(ld)?;
        let (r, _) = build_r_from_eigen(&lambda, q.as_ref(), prune_thresh);
        Ok(r)
    }
}

/// Decompose PLINK genotypes (individuals × n_snps, NA for missing) via the
/// double-scaled SVD. Faithful port of `decompose.ld` plink-mode.
pub fn decompose_plink(genotypes: MatRef<f64>, prune_thresh: f64, max_block_size: usize) -> Result<Mat<f64>> {
    let n_ref = genotypes.nrows();
    let x1 = scale_columns(&genotypes.to_owned());
    // X[is.na(X)] = 0
    let mut x1z = x1;
    for j in 0..x1z.ncols() {
        for i in 0..x1z.nrows() {
            if x1z[(i, j)].is_nan() {
                x1z[(i, j)] = 0.0;
            }
        }
    }
    let x = scale_columns(&x1z);
    // svd(X): lambda = d^2/(N_ref-1), Q = V (right singular vectors)
    let svd = x.as_ref().svd().map_err(|e| LavaError::Numeric(format!("svd failed: {e:?}")))?;
    let d = svd.S(); // singular values (DiagRef)
    let v = svd.V(); // right singular vectors (MatRef), n_snps × p
    let dv = d.column_vector();
    let lambda: Vec<f64> = (0..d.dim()).map(|i| dv[i] * dv[i] / (n_ref - 1) as f64).collect();
    // Q = v (n_snps × p). Use as eigenvectors.
    let (r, _k) = build_r_from_eigen(&lambda, v, prune_thresh);
    // Block path is not applicable to the genotype SVD; for very large loci we
    // rely on the eigen-mode via the LD matrix. (Matches R: the plink branch has
    // no block path — it svd's the whole genotype matrix directly.)
    let _ = max_block_size;
    Ok(r)
}
