//! Helper functions: kernel weights, Vandermonde matrix, matrix utilities.

use faer::linalg::solvers::{DenseSolveCore, Llt, Solve};
use faer::{Mat, Side};

// =====================================================================
// Kernel weight (faithful port of rdrobust_kweight)
// =====================================================================

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kernel {
    Triangular,
    Uniform,
    Epanechnikov,
}

impl Kernel {
    pub fn parse(s: &str) -> Self {
        let s = s.to_lowercase();
        match s.as_str() {
            "epanechnikov" | "epa" => Self::Epanechnikov,
            "uniform" | "uni" => Self::Uniform,
            _ => Self::Triangular,
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::Epanechnikov => "Epanechnikov",
            Self::Uniform => "Uniform",
            Self::Triangular => "Triangular",
        }
    }
}

/// Kernel weight function: faithful port of `rdrobust_kweight(X, c, h, kernel)`.
pub fn kernel_weight(x: &[f64], c: f64, h: f64, kernel: Kernel) -> Vec<f64> {
    x.iter()
        .map(|&xi| {
            let u = (xi - c) / h;
            match kernel {
                Kernel::Epanechnikov => 0.75 * (1.0 - u * u) * (u.abs() <= 1.0) as i32 as f64 / h,
                Kernel::Uniform => 0.5 * (u.abs() <= 1.0) as i32 as f64 / h,
                Kernel::Triangular => (1.0 - u.abs()) * (u.abs() <= 1.0) as i32 as f64 / h,
            }
        })
        .collect()
}

// =====================================================================
// Vandermonde matrix (faithful port of .rdrobust_vander)
// =====================================================================

/// Build Vandermonde matrix `[1, u, u^2, ..., u^p]` (n × (p+1)).
pub fn vandermonde(u: &[f64], p: usize) -> Mat<f64> {
    let n = u.len();
    if p == 0 {
        return Mat::from_fn(n, 1, |_, _| 1.0);
    }
    let mut out = Mat::from_fn(n, p + 1, |_, j| if j == 0 { 1.0 } else { 0.0 });
    for j in 1..=p {
        for i in 0..n {
            out[(i, j)] = out[(i, j - 1)] * u[i];
        }
    }
    out
}

// =====================================================================
// Matrix utilities
// =====================================================================

/// Scale each row of matrix `m` by the corresponding element of `v`.
pub fn scale_rows(m: &Mat<f64>, v: &[f64]) -> Mat<f64> {
    let mut out = m.clone();
    for j in 0..m.ncols() {
        for i in 0..m.nrows() {
            out[(i, j)] *= v[i];
        }
    }
    out
}

/// Scale each column of matrix `m` by the corresponding element of `v`.
pub fn scale_cols(m: &Mat<f64>, v: &[f64]) -> Mat<f64> {
    let mut out = m.clone();
    for j in 0..m.ncols() {
        for i in 0..m.nrows() {
            out[(i, j)] *= v[j];
        }
    }
    out
}

/// `t(a) %*% b` (R: `crossprod(a, b)`).
pub fn crossprod(a: &Mat<f64>, b: &Mat<f64>) -> Mat<f64> {
    a.transpose() * b
}

/// `t(a) %*% a` (R: `crossprod(a)`).
pub fn self_crossprod(a: &Mat<f64>) -> Mat<f64> {
    let at = a.transpose();
    at * a
}

/// QR / XX inverse: `(t(x) %*% x)^{-1}` via Cholesky, with LU fallback.
pub fn qr_xx_inv(x: &Mat<f64>) -> Mat<f64> {
    let g = self_crossprod(x);
    match Llt::new(g.as_ref(), Side::Lower) {
        Ok(llt) => llt.inverse(),
        Err(_) => g.partial_piv_lu().inverse(),
    }
}

/// Compute `t(R) %*% diag(w) %*% D` efficiently.
pub fn weighted_crossprod(r: &Mat<f64>, w: &[f64], d: &Mat<f64>) -> Mat<f64> {
    let nr = r.nrows();
    let nc_r = r.ncols();
    let nc_d = d.ncols();
    let mut out = Mat::zeros(nc_r, nc_d);
    for k in 0..nr {
        let wk = w[k];
        for j in 0..nc_d {
            let dkj = d[(k, j)];
            for i in 0..nc_r {
                out[(i, j)] += r[(k, i)] * wk * dkj;
            }
        }
    }
    out
}

/// Row sums of `(R %*% invG) * (R * w)`.
pub fn hat_values(r: &Mat<f64>, inv_g: &Mat<f64>, w: &[f64]) -> Vec<f64> {
    let rg = r * inv_g;
    let n = r.nrows();
    let k = r.ncols();
    let mut out = vec![0.0; n];
    for i in 0..n {
        let wi = w[i];
        let mut s = 0.0;
        for j in 0..k {
            s += rg[(i, j)] * r[(i, j)] * wi;
        }
        out[i] = s;
    }
    out
}

/// Factorial for small non-negative integers.
pub fn factorial(n: usize) -> f64 {
    (1..=n).product::<usize>() as f64
}

/// R's `rle` for a sorted vector.
pub fn rle(x: &[f64]) -> (Vec<usize>, Vec<f64>) {
    let n = x.len();
    if n == 0 {
        return (vec![], vec![]);
    }
    let mut lengths = vec![];
    let mut values = vec![];
    let mut current = x[0];
    let mut count = 1;
    for i in 1..n {
        if x[i] == current {
            count += 1;
        } else {
            lengths.push(count);
            values.push(current);
            current = x[i];
            count = 1;
        }
    }
    lengths.push(count);
    values.push(current);
    (lengths, values)
}

/// Build `dups` and `dupsid` arrays from sorted x.
pub fn dups_dupsid(x: &[f64]) -> (Vec<usize>, Vec<usize>) {
    let (lengths, _) = rle(x);
    let mut dups = vec![0usize; x.len()];
    let mut dupsid = vec![0usize; x.len()];
    let mut idx = 0;
    for &len in &lengths {
        for j in 0..len {
            dups[idx] = len;
            dupsid[idx] = j + 1;
            idx += 1;
        }
    }
    (dups, dupsid)
}

/// Build a cluster index: list of observation indices per cluster.
pub fn cluster_idx(c: &[f64]) -> Vec<Vec<usize>> {
    let n = c.len();
    if n == 0 {
        return vec![];
    }
    let mut ord: Vec<usize> = (0..n).collect();
    ord.sort_by(|&a, &b| c[a].partial_cmp(&c[b]).unwrap_or(std::cmp::Ordering::Equal));
    let mut out = vec![];
    let mut start = 0;
    while start < n {
        let mut end = start + 1;
        while end < n && c[ord[end]] == c[ord[start]] {
            end += 1;
        }
        out.push(ord[start..end].to_vec());
        start = end;
    }
    out
}

/// General matrix inverse via LU (for non-SPD matrices).
pub fn lu_inverse(m: &Mat<f64>) -> Mat<f64> {
    m.partial_piv_lu().inverse()
}

/// SPD matrix inverse via Cholesky. Returns None if not SPD.
pub fn spd_inverse(m: &Mat<f64>) -> Option<Mat<f64>> {
    Llt::new(m.as_ref(), Side::Lower)
        .ok()
        .map(|llt| llt.inverse())
}

/// Symmetric eigendecomposition. Returns (eigenvalues_ascending, eigenvectors).
pub fn sym_eigen(m: &Mat<f64>) -> Option<(Vec<f64>, Mat<f64>)> {
    let e = m.as_ref().self_adjoint_eigen(Side::Lower).ok()?;
    let s = e.S();
    let u = e.U();
    let n = m.nrows();
    let sv = s.column_vector();
    let vals: Vec<f64> = (0..n).map(|i| sv[i]).collect();
    let vecs = Mat::from_fn(n, n, |row, col| u[(row, col)]);
    Some((vals, vecs))
}
