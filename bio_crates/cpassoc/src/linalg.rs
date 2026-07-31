//! Linear-algebra helpers: Moore-Penrose pseudoinverse (`ginv`) and small
//! matrix utilities, faithful to R's `MASS::ginv`.
//!
//! R's `ginv` computes the Moore-Penrose pseudoinverse via SVD with tolerance
//! `sqrt(.Machine$double.eps) * max(dim) * sigma_max`. We reproduce that here
//! using [`faer`].

use faer::{Mat, MatRef};

/// R's `.Machine$double.eps^0.5` ≈ 1.49e-8.
const DBL_EPS_SQRT: f64 = 1.4901161193847656e-8;

/// Moore-Penrose pseudoinverse, faithful to R's `MASS::ginv`.
///
/// Returns `A⁺` where singular values below
/// `max(dim(A)) * sqrt(.Machine$double.eps) * σ_max` are zeroed.
pub fn ginv(a: MatRef<f64>) -> Mat<f64> {
    let m = a.nrows();
    let n = a.ncols();
    if m == 0 || n == 0 {
        return Mat::zeros(n, m);
    }
    let svd = match a.svd() {
        Ok(s) => s,
        // ginv never errors in R (it just gives a result); fall back to zeros.
        Err(_) => return Mat::zeros(n, m),
    };
    let u = svd.U(); // m × k
    let v = svd.V(); // n × k  (faer stores V, not Vᵀ)
    let dv = svd.S().column_vector(); // singular values, length k
    let k = u.ncols().min(v.ncols()); // rank dimension
    let max_dim = m.max(n) as f64;
    let tol = max_dim * DBL_EPS_SQRT * dv[0]; // R: tol = max(dim) * sqrt(.Machine$double.eps) * d[1]
    let mut d_plus = vec![0.0_f64; k];
    for i in 0..k {
        if dv[i] > tol {
            d_plus[i] = 1.0 / dv[i];
        }
    }
    // A⁺ = V · diag(d⁺) · Uᵀ  →  result[j, i] = Σ_l V[j,l] · d⁺[l] · U[i,l]
    let mut result = Mat::zeros(n, m);
    for j in 0..n {
        for i in 0..m {
            let mut s = 0.0;
            for l in 0..k {
                s += v[(j, l)] * d_plus[l] * u[(i, l)];
            }
            result[(j, i)] = s;
        }
    }
    result
}

/// Compute `wᵀ A⁺ x` and `wᵀ A⁺ w` in one pass, where `A⁺` is `ginv(A)`.
///
/// Returns `(numerator_base, denominator)` where:
/// - `numerator_base = wᵀ A⁺ x`  (a scalar)
/// - `denominator    = wᵀ A⁺ w`  (a scalar)
///
/// This is the core of both SHom and SHet. The test statistic is
/// `(numerator_base²) / denominator`.
pub fn weighted_score(a: MatRef<f64>, w: &[f64], x: &[f64]) -> (f64, f64) {
    let a_inv = ginv(a);
    let k = a_inv.nrows();
    // A⁺ x
    let mut ax = vec![0.0_f64; k];
    for i in 0..k {
        let mut s = 0.0;
        for j in 0..x.len() {
            s += a_inv[(i, j)] * x[j];
        }
        ax[i] = s;
    }
    // w · (A⁺ x)
    let num: f64 = w.iter().zip(&ax).map(|(wi, ai)| wi * ai).sum();
    // A⁺ w
    let mut aw = vec![0.0_f64; k];
    for i in 0..k {
        let mut s = 0.0;
        for j in 0..w.len() {
            s += a_inv[(i, j)] * w[j];
        }
        aw[i] = s;
    }
    // w · (A⁺ w)
    let den: f64 = w.iter().zip(&aw).map(|(wi, ai)| wi * ai).sum();
    (num, den)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ginv_identity() {
        let a = Mat::from_fn(3, 3, |i, j| if i == j { 2.0 } else { 0.0 });
        let pinv = ginv(a.as_ref());
        for i in 0..3 {
            assert!((pinv[(i, i)] - 0.5).abs() < 1e-12);
        }
    }

    #[test]
    fn ginv_singular() {
        // Rank-1 matrix: [[1,2],[2,4]] — pseudoinverse should be [[0.04,0.08],[0.08,0.16]]
        let a = Mat::from_fn(2, 2, |i, j| ((i + 1) * (j + 1)) as f64);
        // a = [[1,2],[2,4]]
        assert!((a[(0, 0)] - 1.0).abs() < 1e-15);
        assert!((a[(1, 1)] - 4.0).abs() < 1e-15);
        let pinv = ginv(a.as_ref());
        assert_eq!(pinv.nrows(), 2);
        assert_eq!(pinv.ncols(), 2);
        // Pseudoinverse of [[1,2],[2,4]]: singular value σ=5, A⁺ = Aᵀ/125
        // = [[1,2],[2,4]]ᵀ / 25 ... actually A = u*vᵀ where u=[1,2], v=[1,2]
        // A⁺ = v*uᵀ / (||u||² * ||v||²) = [1,2]*[1,2]ᵀ / (5*5) = [[1,2],[2,4]]/25
        assert!((pinv[(0, 0)] - 0.04).abs() < 1e-10, "pinv[0,0]={}", pinv[(0, 0)]);
        assert!((pinv[(0, 1)] - 0.08).abs() < 1e-10, "pinv[0,1]={}", pinv[(0, 1)]);
        assert!((pinv[(1, 0)] - 0.08).abs() < 1e-10, "pinv[1,0]={}", pinv[(1, 0)]);
        assert!((pinv[(1, 1)] - 0.16).abs() < 1e-10, "pinv[1,1]={}", pinv[(1, 1)]);
    }

    #[test]
    fn ginv_symmetric_pd() {
        // For a symmetric PD matrix, ginv should equal the true inverse
        let a = Mat::from_fn(2, 2, |i, j| match (i, j) {
            (0, 0) => 2.0,
            (0, 1) => 1.0,
            (1, 0) => 1.0,
            (1, 1) => 3.0,
            _ => unreachable!(),
        });
        let pinv = ginv(a.as_ref());
        // Inverse of [[2,1],[1,3]] = 1/5 * [[3,-1],[-1,2]] = [[0.6,-0.2],[-0.2,0.4]]
        assert!((pinv[(0, 0)] - 0.6).abs() < 1e-10);
        assert!((pinv[(0, 1)] - (-0.2)).abs() < 1e-10);
        assert!((pinv[(1, 0)] - (-0.2)).abs() < 1e-10);
        assert!((pinv[(1, 1)] - 0.4).abs() < 1e-10);
    }
}
