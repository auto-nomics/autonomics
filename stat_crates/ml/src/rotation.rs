//! Factor-loading rotation — varimax (orthogonal) and promax (oblique).
//!
//! Faithful ports of R's `stats::varimax` / `stats::promax` (factanal.R):
//! given a p×k loading matrix (p variables × k factors), varimax maximises
//! the Kaiser criterion Σ_j Σ_i (λ²ᵢⱼ − Σᵢ' λ²ᵢ'ⱼ/p)² by successive
//! pairwise-free SVD updates; promax raises the varimax loadings to a power
//! `m`, least-squares-fits that oblique target, and rescales `U` so the
//! rotated factors keep unit variance. R defaults are preserved:
//! `varimax(normalize = TRUE, eps = 1e-5)` and `promax(m = 4)` (promax
//! hard-codes the internal varimax normalisation).
//!
//! Golden parity: `tests/rotation_r_reference.rs` against `stats::varimax` /
//! `stats::promax` on a fixed loading matrix.

use faer::Mat;
use faer::linalg::solvers::{Llt, Solve};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum RotationError {
    #[error("loading matrix must have ≥ 2 columns, got {0}")]
    TooFewFactors(usize),
    #[error("empty loading matrix")]
    Empty,
    #[error("numerical error: {0}")]
    Numeric(String),
}

pub type Result<T> = std::result::Result<T, RotationError>;

/// Result of a factor rotation.
#[derive(Debug, Clone)]
pub struct RotationResult {
    /// Rotated loadings (p × k).
    pub loadings: Mat<f64>,
    /// Rotation matrix (k × k). Orthogonal for varimax; oblique for promax —
    /// R returns the same object (`rotmat`).
    pub rotmat: Mat<f64>,
}

/// Orthogonal varimax rotation (R `stats::varimax`).
///
/// - `normalize`: Kaiser row normalisation before rotation (rows rescaled
///   back afterwards); R default `TRUE`.
/// - `eps`: convergence threshold on the relative change of the criterion
///   (sum of singular values); R default `1e-5`.
///
/// With fewer than two factors the input is returned unchanged with an
/// identity rotation (R does the same).
pub fn varimax(x: &Mat<f64>, normalize: bool, eps: f64) -> Result<RotationResult> {
    let (p, k) = x.shape();
    if p == 0 || k == 0 {
        return Err(RotationError::Empty);
    }
    if k < 2 {
        return Ok(RotationResult {
            loadings: x.clone(),
            rotmat: Mat::identity(k, k),
        });
    }

    // Kaiser normalisation: divide each row by its square-root sum of squares.
    let sc: Vec<f64> = (0..p)
        .map(|i| (0..k).map(|j| x[(i, j)] * x[(i, j)]).sum::<f64>().sqrt())
        .collect();
    let work: Mat<f64> = if normalize {
        Mat::from_fn(p, k, |i, j| {
            let s = sc[i];
            if s > 0.0 { x[(i, j)] / s } else { x[(i, j)] }
        })
    } else {
        x.clone()
    };

    // Iterate: TT ← u·vᵀ from the SVD of B = xᵀ(z³ − z·diag(colsum z²)/p),
    // converge when d < dpast·(1 + eps) with d = Σ singular values
    // (R: for i in 1:1000, then z <- x %*% TT with the final TT).
    let mut tt = Mat::identity(k, k);
    let mut d_past = 0.0_f64;
    for _ in 0..1000 {
        let z = &work * &tt;
        // Column sums of z² (variance per factor).
        let z2_colsum: Vec<f64> = (0..k)
            .map(|j| (0..p).map(|i| z[(i, j)] * z[(i, j)]).sum::<f64>())
            .collect();
        // M = z³ − z·diag(colsum)/p   (p × k, built from z alone), then
        // B = xᵀ·M  (k × k).
        let m = Mat::from_fn(p, k, |i, j| {
            let z_ij = z[(i, j)];
            z_ij * z_ij * z_ij - z_ij * z2_colsum[j] / p as f64
        });
        let b = work.transpose() * &m;
        let svd = b
            .svd()
            .map_err(|e| RotationError::Numeric(format!("varimax SVD: {e:?}")))?;
        let d: f64 = svd.S().column_vector().iter().sum();
        // TT = u·vᵀ
        tt = Mat::from_fn(k, k, |i, j| {
            let mut s = 0.0;
            for mm in 0..k {
                s += svd.U()[(i, mm)] * svd.V()[(j, mm)];
            }
            s
        });
        if d < d_past * (1.0 + eps) {
            break;
        }
        d_past = d;
    }
    let z = &work * &tt;

    // Un-normalise and return.
    let loadings = Mat::from_fn(p, k, |i, j| {
        let v = z[(i, j)];
        if normalize && sc[i] > 0.0 {
            v * sc[i]
        } else {
            v
        }
    });
    Ok(RotationResult {
        loadings,
        rotmat: tt,
    })
}

/// Oblique promax rotation (R `stats::promax`).
///
/// Runs `varimax` internally (with R's hard-coded `normalize = TRUE`,
/// `eps = 1e-5`), builds the target `Q = loadings ∘ |loadings|^(m−1)`,
/// least-squares-fits `U = (xᵀx)⁻¹xᵀQ`, rescales `U`'s columns by
/// `sqrt(diag((UᵀU)⁻¹))`, and returns `z = x·U` with the accumulated
/// `rotmat = varimax.rotmat · U`. R default `m = 4`.
pub fn promax(x: &Mat<f64>, m: f64) -> Result<RotationResult> {
    let (p, k) = x.shape();
    if p == 0 || k == 0 {
        return Err(RotationError::Empty);
    }
    if k < 2 {
        return Ok(RotationResult {
            loadings: x.clone(),
            rotmat: Mat::identity(k, k),
        });
    }

    let xx = varimax(x, true, 1e-5)?;
    let v = &xx.loadings;

    // Q = v ∘ |v|^(m−1)  (sign preserved).
    let q = Mat::from_fn(p, k, |i, j| {
        let a = v[(i, j)];
        a * a.abs().powf(m - 1.0)
    });

    // U = (vᵀv)⁻¹·vᵀ·Q — solve the k×k normal equations column by column.
    let vtv = v.transpose() * v;
    let vty = v.transpose() * &q;
    let mut u = Mat::zeros(k, k);
    for j in 0..k {
        let rhs = Mat::from_fn(k, 1, |i, _| vty[(i, j)]);
        let sol = solve_spd(&vtv, &rhs)?;
        for i in 0..k {
            u[(i, j)] = sol[(i, 0)];
        }
    }

    // Column rescale: U ← U·diag(sqrt(diag((UᵀU)⁻¹))) — the diagonal of the
    // inverse, i.e. solve UᵀU·x = e_j and read x[j] for each j.
    let utu = u.transpose() * &u;
    let mut scale = vec![0.0_f64; k];
    for j in 0..k {
        let e_j = Mat::from_fn(k, 1, |i, _| if i == j { 1.0 } else { 0.0 });
        let sol = solve_spd(&utu, &e_j)?;
        scale[j] = sol[(j, 0)].max(0.0).sqrt();
    }
    let u = Mat::from_fn(k, k, |i, j| u[(i, j)] * scale[j]);

    // z = v·U ; rotmat = varimax.rotmat · U.
    let loadings = v * &u;
    let rotmat = &xx.rotmat * &u;
    Ok(RotationResult { loadings, rotmat })
}

/// Solve SPD `a·x = b` via faer Cholesky.
fn solve_spd(a: &Mat<f64>, b: &Mat<f64>) -> Result<Mat<f64>> {
    let llt = Llt::new(a.as_ref(), faer::Side::Lower)
        .ok()
        .ok_or_else(|| RotationError::Numeric("singular promax system".into()))?;
    Ok(llt.solve(b))
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// A simple 6×2 loading matrix with two correlated factors.
    fn loadings2() -> Mat<f64> {
        Mat::from_fn(6, 2, |i, j| match (i, j) {
            (0, _) => 0.9 - 0.05 * i as f64,
            (1, _) => 0.8 + 0.03 * i as f64,
            (2, 0) => 0.1,
            (2, 1) => 0.7,
            (3, 0) => 0.15,
            (3, 1) => 0.65,
            (4, 0) => 0.85,
            (4, 1) => 0.2,
            _ => 0.7 + 0.02 * i as f64 - 0.4 * j as f64,
        })
    }

    fn assert_orthonormal(m: &Mat<f64>, what: &str) {
        let k = m.nrows();
        for i in 0..k {
            for j in 0..k {
                let dot: f64 = (0..k).map(|t| m[(t, i)] * m[(t, j)]).sum();
                let expect = if i == j { 1.0 } else { 0.0 };
                assert!(
                    (dot - expect).abs() < 1e-9,
                    "{what} not orthonormal at ({i},{j}): {dot}"
                );
            }
        }
    }

    #[test]
    fn single_factor_is_identity() {
        let x = Mat::from_fn(5, 1, |i, _| 0.3 + 0.1 * i as f64);
        let r = varimax(&x, true, 1e-5).unwrap();
        assert_eq!(r.loadings.shape(), (5, 1));
        assert!((r.rotmat[(0, 0)] - 1.0).abs() < 1e-12);
        let p = promax(&x, 4.0).unwrap();
        assert!((p.rotmat[(0, 0)] - 1.0).abs() < 1e-12);
    }

    #[test]
    fn varimax_rotmat_is_orthogonal_and_sums_preserved() {
        let x = loadings2();
        let r = varimax(&x, true, 1e-5).unwrap();
        assert_orthonormal(&r.rotmat, "varimax rotmat");
        // Total sums of squares are preserved by an orthogonal rotation.
        let ssq = |m: &Mat<f64>| {
            let (rows, cols) = m.shape();
            (0..rows)
                .flat_map(|i| (0..cols).map(move |j| m[(i, j)] * m[(i, j)]))
                .sum::<f64>()
        };
        let ss_before = ssq(&x);
        let ss_after = ssq(&r.loadings);
        assert!((ss_before - ss_after).abs() < 1e-9);
    }

    #[test]
    fn promax_m2_close_to_varimax() {
        // m → 2 makes the target nearly the (already varimax-rotated)
        // loadings, so the oblique rotation stays close to orthogonal.
        let x = loadings2();
        let v = varimax(&x, true, 1e-5).unwrap();
        let p = promax(&x, 2.0).unwrap();
        let diff = |a: &Mat<f64>, b: &Mat<f64>| {
            let (rows, cols) = a.shape();
            (0..rows)
                .flat_map(|i| (0..cols).map(move |j| (a[(i, j)] - b[(i, j)]).abs()))
                .fold(0.0_f64, f64::max)
        };
        assert!(
            diff(&v.loadings, &p.loadings) < 0.15,
            "promax m=2 drifted from varimax: {}",
            diff(&v.loadings, &p.loadings)
        );
    }

    #[test]
    fn normalisation_roundtrip_matches_unnormalised_scale() {
        // Rotated loadings come back on the original row scale.
        let x = loadings2();
        let r = varimax(&x, true, 1e-5).unwrap();
        let row_norm = |m: &Mat<f64>, i: usize| {
            (0..m.ncols())
                .map(|j| m[(i, j)] * m[(i, j)])
                .sum::<f64>()
                .sqrt()
        };
        for i in 0..x.nrows() {
            assert!(
                (row_norm(&x, i) - row_norm(&r.loadings, i)).abs() < 1e-9,
                "row {i} scale changed"
            );
        }
    }
}
