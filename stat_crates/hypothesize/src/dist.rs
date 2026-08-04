//! Distribution helpers — thin R-named wrappers over `statrs`.
//!
//! All functions follow R's lower-tail / upper-tail convention via explicit
//! `_cdf` (lower tail, `P(X ≤ x)`) and `_sf` (upper tail, `P(X > x)`)
//! suffixes, mirroring `pnorm(..., lower.tail=FALSE)` etc.

use statrs::distribution::{ChiSquared, ContinuousCDF, FisherSnedecor, Normal, StudentsT};

use crate::{HypoError, Result};

/// Standard-normal CDF `Φ(x)` (= `pnorm(x)`).
pub fn normal_cdf(x: f64) -> f64 {
    Normal::standard().cdf(x)
}

/// Standard-normal survival function `1 − Φ(x)` (= `pnorm(x, lower.tail=FALSE)`).
pub fn normal_sf(x: f64) -> f64 {
    Normal::standard().sf(x)
}

/// Standard-normal inverse CDF `Φ⁻¹(p)` (= `qnorm(p)`).
pub fn normal_inv(p: f64) -> f64 {
    Normal::standard().inverse_cdf(p)
}

/// Two-sided normal p-value `2·Φ(−|z|)`.
pub fn normal_two_sided_p(z: f64) -> f64 {
    2.0 * normal_sf(z.abs())
}

/// χ² upper-tail `P(X > x)` for `df` degrees of freedom
/// (= `pchisq(x, df, lower.tail=FALSE)`).
pub fn chisq_sf(x: f64, df: f64) -> f64 {
    if df <= 0.0 {
        return f64::NAN;
    }
    match ChiSquared::new(df) {
        Ok(d) => d.sf(x),
        Err(_) => f64::NAN,
    }
}

/// Student-t upper-tail `P(T > x)` for `df` degrees of freedom
/// (= `pt(x, df, lower.tail=FALSE)`).
pub fn t_sf(x: f64, df: f64) -> f64 {
    if df <= 0.0 {
        return f64::NAN;
    }
    match StudentsT::new(0.0, 1.0, df) {
        Ok(d) => d.sf(x),
        Err(_) => f64::NAN,
    }
}

/// Two-sided t p-value `2·P(T > |x|)`.
pub fn t_two_sided_p(x: f64, df: f64) -> f64 {
    2.0 * t_sf(x.abs(), df)
}

/// F upper-tail `P(F > x)` for `(d1, d2)` degrees of freedom
/// (= `pf(x, d1, d2, lower.tail=FALSE)`).
pub fn f_sf(x: f64, d1: f64, d2: f64) -> f64 {
    if d1 <= 0.0 || d2 <= 0.0 {
        return f64::NAN;
    }
    match FisherSnedecor::new(d1, d2) {
        Ok(d) => d.sf(x),
        Err(_) => f64::NAN,
    }
}

/// Solve `inv(matrix) · v` for a symmetric positive-definite `matrix`.
///
/// Uses faer's `Llt` (Cholesky on the lower triangle); returns
/// [`HypoError::SingularMatrix`] on factorisation failure, mirroring R's
/// `solve()` error for non-PD inputs.
pub(crate) fn solve_spd(matrix: &[Vec<f64>], v: &[f64]) -> Result<Vec<f64>> {
    use faer::linalg::solvers::{Llt, Solve};
    use faer::Side;

    let k = matrix.len();
    if k == 0 {
        return Err(HypoError::InvalidInput("empty matrix".into()));
    }
    for row in matrix {
        if row.len() != k {
            return Err(HypoError::InvalidInput(format!(
                "matrix is not square: row of length {} in {k}×? matrix",
                row.len()
            )));
        }
    }
    if v.len() != k {
        return Err(HypoError::LengthMismatch { a: k, b: v.len() });
    }

    let mat = faer::Mat::from_fn(k, k, |i, j| matrix[i][j]);
    let rhs = faer::Mat::from_fn(k, 1, |i, _| v[i]);
    let llt = Llt::new(mat.as_ref(), Side::Lower)
        .ok()
        .ok_or_else(|| HypoError::SingularMatrix("matrix is not positive-definite".into()))?;
    let sol = llt.solve(&rhs);
    Ok((0..k).map(|i| sol[(i, 0)]).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normal_inv_roundtrip() {
        for p in [0.01, 0.05, 0.5, 0.95, 0.99] {
            let z = normal_inv(p);
            assert!((normal_cdf(z) - p).abs() < 1e-10, "p={p}");
        }
    }

    #[test]
    fn chisq_sf_matches_known() {
        // pchisq(3.84, 1, lower.tail=FALSE) ≈ 0.05
        assert!((chisq_sf(3.841459, 1.0) - 0.05).abs() < 1e-5);
        // pchisq(5.991, 2, lower.tail=FALSE) ≈ 0.05
        assert!((chisq_sf(5.991465, 2.0) - 0.05).abs() < 1e-5);
    }

    #[test]
    fn f_sf_matches_known() {
        // R: pf(4.468342, 4, 10, lower.tail=FALSE) ≈ 0.025
        assert!((f_sf(4.468_342, 4.0, 10.0) - 0.025).abs() < 1e-3);
        // R: pf(4.7472, 4, 10, lower.tail=FALSE) ≈ 0.02088
        assert!((f_sf(4.7472, 4.0, 10.0) - 0.020_884).abs() < 1e-4);
    }

    #[test]
    fn solve_spd_identity() {
        let v = vec![vec![1.0, 0.0], vec![0.0, 1.0]];
        let x = solve_spd(&v, &[2.0, 3.0]).unwrap();
        assert!((x[0] - 2.0).abs() < 1e-10 && (x[1] - 3.0).abs() < 1e-10);
    }

    #[test]
    fn solve_spd_singular_rejected() {
        let v = vec![vec![1.0, 1.0], vec![1.0, 1.0]]; // rank-1
        assert!(solve_spd(&v, &[1.0, 1.0]).is_err());
    }
}
