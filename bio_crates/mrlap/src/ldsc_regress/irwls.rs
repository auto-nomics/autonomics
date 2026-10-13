//! Iteratively reweighted least squares — a faithful port of LDSC's
//! `IRWLS` (`ldscore/irwls.py`) together with the `Hsq.weights` reweighting
//! function (`ldscore/regressions.py:497-535`).
//!
//! The flow mirrors the Python exactly:
//! 1. Seed weights from the initial aggregate h².
//! 2. Two reweighting passes: weighted least squares → derive h² & intercept
//!    from the coefficients → recompute weights via [`hsq_weights`].
//! 3. Apply the final `√w` row-scaling (LDSC's `_weight`) to produce the
//!    weighted design/response that the block jackknife then operates on.
//!
//! **Equivalence note.** LDSC's `wls` scales each row by `√w / Σ√w` (with `w`
//! on the inverse-CVF scale) and runs a plain `lstsq`. The global `1/Σ√w`
//! factor is a positive constant multiplying every effective weight equally,
//! so the resulting β is identical to standard weighted least squares with
//! weights `w`. We therefore implement the per-iteration solve with
//! [`super::linalg::wls`] (which solves `(Xᵀ W X) β = Xᵀ W y`) and only apply
//! the row-scaling once, for the final jackknife input — that scaling, too, is
//! globally constant and cancels in every jackknife quantity (coef, pseudovalues,
//! covariance), so its omission changes nothing numerically.

use faer::Mat;

use super::jackknife::{JackknifeResult, jackknife_fast};
use super::{LdscError, Result};

use super::linalg::wls;

/// Per-SNP Hsq regression weights — the reciprocal of the conditional variance
/// function. Port of `Hsq.weights` (regressions.py:497-535).
///
/// `wⱼ = 1 / (2 · w_ldⱼ · (intercept + hsq·Nⱼ·ldⱼ/M)²)`, with `ld` and `w_ld`
/// floored at 1 and `hsq` clipped to `[0, 1]`.
pub fn hsq_weights(
    ld: &[f64],
    w_ld: &[f64],
    n_samples: &[f64],
    m_tot: f64,
    hsq: f64,
    intercept: f64,
) -> Vec<f64> {
    let hsq = hsq.clamp(0.0, 1.0);
    (0..ld.len())
        .map(|i| {
            let ldi = ld[i].max(1.0);
            let wldi = w_ld[i].max(1.0);
            let c = hsq * n_samples[i] / m_tot;
            let het_w = 1.0 / (2.0 * (intercept + c * ldi).powi(2));
            let oc_w = 1.0 / wldi;
            het_w * oc_w
        })
        .collect()
}

/// Output of [`irwls`]: the weighted design matrix and response ready to hand
/// to [`super::jackknife::jackknife_fast`].
pub struct IrwlsOutput {
    /// Row-scaled design matrix (`n × p`).
    pub x: Mat<f64>,
    /// Row-scaled response (length `n`).
    pub y: Vec<f64>,
}

/// Per-SNP Gencov regression weights — port of `Gencov.weights`
/// (`regressions.py:621-677`). `w = 1/(w_ld·(a·b + c²))` with
/// `a = N1·h1·ld/M + int1`, `b = N2·h2·ld/M + int2`,
/// `c = √(N1·N2)·ρg·ld/M + int_gencov`.
pub fn gencov_weights(
    ld: &[f64],
    w_ld: &[f64],
    n1: &[f64],
    n2: &[f64],
    m_tot: f64,
    h1: f64,
    h2: f64,
    rho_g: f64,
    intercept_gencov: f64,
    intercept_hsq1: f64,
    intercept_hsq2: f64,
) -> Vec<f64> {
    let h1 = h1.clamp(0.0, 1.0);
    let h2 = h2.clamp(0.0, 1.0);
    let rho_g = rho_g.clamp(-1.0, 1.0);
    (0..ld.len())
        .map(|i| {
            let ldi = ld[i].max(1.0);
            let wldi = w_ld[i].max(1.0);
            let a = n1[i] * h1 * ldi / m_tot + intercept_hsq1;
            let b = n2[i] * h2 * ldi / m_tot + intercept_hsq2;
            let sqrt_n1n2 = (n1[i] * n2[i]).sqrt();
            let c = sqrt_n1n2 * rho_g * ldi / m_tot + intercept_gencov;
            let het_w = 1.0 / (a * b + c * c);
            let oc_w = 1.0 / wldi;
            het_w * oc_w
        })
        .collect()
}

/// Run the two-pass IRWLS reweighting.
///
/// # Arguments
/// * `x` — design matrix (`n × p`). LD columns first (N-scaled), intercept
///   column last when `free_intercept` is true.
/// * `y` — response (χ² for Hsq; χ² − intercept when constrained).
/// * `ld_tot` — per-SNP **total** LD (sum of raw annotation LD scores), length
///   `n`. The weight formula uses the raw total LD, not the N-scaled columns.
/// * `w_ld`, `n_samples`, `m_tot`, `nbar` — weight-formula / scaling inputs.
/// * `n_annot` — number of LD annotation columns.
/// * `free_intercept` — whether the design carries an intercept column.
/// * `initial_hsq`, `intercept` — seeds for the first weight computation.
pub fn irwls(
    x: &Mat<f64>,
    y: &[f64],
    ld_tot: &[f64],
    w_ld: &[f64],
    n_samples: &[f64],
    m_tot: f64,
    nbar: f64,
    n_annot: usize,
    free_intercept: bool,
    initial_hsq: f64,
    mut intercept: f64,
) -> Result<IrwlsOutput> {
    let n = y.len();
    if x.nrows() != n
        || ld_tot.len() != n
        || w_ld.len() != n
        || n_samples.len() != n
        || x.ncols() < n_annot
    {
        return Err(LdscError::DimensionMismatch(
            "irwls: dimension mismatch".into(),
        ));
    }
    let p = x.ncols();

    let mut hsq = initial_hsq;
    let mut raw = hsq_weights(ld_tot, w_ld, n_samples, m_tot, hsq, intercept);

    // Exactly two reweighting passes (irwls.py:112).
    for _ in 0..2 {
        let coef = wls(x.as_ref(), y, &raw)?;
        // LDSC derives the weighting h² from coef[0] (regressions.py:364).
        hsq = (m_tot * coef[0] / nbar).clamp(0.0, 1.0);
        if free_intercept && p == n_annot + 1 {
            intercept = coef[n_annot];
        }
        raw = hsq_weights(ld_tot, w_ld, n_samples, m_tot, hsq, intercept);
    }

    // Final √w row-scaling (LDSC `_weight`, with the global normalisation
    // dropped — see the module-level equivalence note).
    let sqrtw: Vec<f64> = raw
        .iter()
        .map(|&w| if w > 0.0 { w.sqrt() } else { 0.0 })
        .collect();
    let xw = Mat::from_fn(n, p, |i, j| x[(i, j)] * sqrtw[i]);
    let yw: Vec<f64> = (0..n).map(|i| y[i] * sqrtw[i]).collect();

    Ok(IrwlsOutput { x: xw, y: yw })
}

/// Generalized IRWLS → block jackknife. Performs exactly two reweighting passes
/// (LDSC `IRWLS.irwls`), where `update(coef)` returns the new inverse-CVF
/// weight vector for the current least-squares coefficients, then hands the
/// `√w`-scaled design/response to [`jackknife_fast`].
///
/// This is the engine behind [`super::regress`]'s Hsq/Gencov regressions.
pub fn irwls_jackknife<F>(
    x: &Mat<f64>,
    y: &[f64],
    n_blocks: usize,
    initial_w: &[f64],
    mut update: F,
) -> Result<JackknifeResult>
where
    F: FnMut(&[f64]) -> Vec<f64>,
{
    let n = y.len();
    if x.nrows() != n || initial_w.len() != n {
        return Err(LdscError::DimensionMismatch(
            "irwls_jackknife: mismatch".into(),
        ));
    }
    let mut w = initial_w.to_vec();
    for _ in 0..2 {
        let coef = wls(x.as_ref(), y, &w)?;
        w = update(&coef);
    }
    let sqrtw: Vec<f64> = w
        .iter()
        .map(|&wi| if wi > 0.0 { wi.sqrt() } else { 0.0 })
        .collect();
    let p = x.ncols();
    let xw = Mat::from_fn(n, p, |i, j| x[(i, j)] * sqrtw[i]);
    let yw: Vec<f64> = (0..n).map(|i| y[i] * sqrtw[i]).collect();
    jackknife_fast(&xw, &yw, n_blocks)
}

/// Like [`irwls_jackknife`] but with explicit jackknife block separators (used
/// by the two-step estimator).
pub fn irwls_jackknife_with_separators<F>(
    x: &Mat<f64>,
    y: &[f64],
    initial_w: &[f64],
    mut update: F,
    separators: &[usize],
) -> Result<JackknifeResult>
where
    F: FnMut(&[f64]) -> Vec<f64>,
{
    let n = y.len();
    if x.nrows() != n || initial_w.len() != n {
        return Err(LdscError::DimensionMismatch(
            "irwls_jackknife_with_separators: mismatch".into(),
        ));
    }
    let mut w = initial_w.to_vec();
    for _ in 0..2 {
        let coef = wls(x.as_ref(), y, &w)?;
        w = update(&coef);
    }
    let sqrtw: Vec<f64> = w
        .iter()
        .map(|&wi| if wi > 0.0 { wi.sqrt() } else { 0.0 })
        .collect();
    let p = x.ncols();
    let xw = Mat::from_fn(n, p, |i, j| x[(i, j)] * sqrtw[i]);
    let yw: Vec<f64> = (0..n).map(|i| y[i] * sqrtw[i]).collect();
    super::jackknife::jackknife_fast_with_separators(&xw, &yw, separators)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ldsc_regress::linalg::build_mat_row_major;

    fn approx(a: f64, b: f64, tol: f64) -> bool {
        (a - b).abs() <= tol * b.abs().max(1.0)
    }

    #[test]
    fn weights_exact_value() {
        // hsq=0.3, N=10000, M=5000, ld=2, w_ld=4, intercept=1.
        // c = 0.3*10000/5000 = 0.6; het_w = 1/(2*(1+0.6*2)^2) = 1/(2*2.2^2)
        let w = hsq_weights(&[2.0], &[4.0], &[10000.0], 5000.0, 0.3, 1.0);
        let base: f64 = 1.0 + 0.6 * 2.0;
        let expected: f64 = 1.0 / (2.0 * base * base) / 4.0;
        assert!(approx(w[0], expected, 1e-12));
    }

    #[test]
    fn weights_floors_ld_and_wld_at_one() {
        // ld=0.1 < 1 → floored; w_ld=0.5 < 1 → floored.
        let w_floor = hsq_weights(&[0.1], &[0.5], &[100.0], 10.0, 0.5, 1.0);
        let w_one = hsq_weights(&[1.0], &[1.0], &[100.0], 10.0, 0.5, 1.0);
        assert!(approx(w_floor[0], w_one[0], 1e-12));
    }

    #[test]
    fn weights_clip_hsq_to_unit() {
        // hsq=5 clipped to 1 → same as hsq=1.
        let w_clip = hsq_weights(&[2.0], &[1.0], &[1000.0], 1000.0, 5.0, 1.0);
        let w_one = hsq_weights(&[2.0], &[1.0], &[1000.0], 1000.0, 1.0, 1.0);
        assert!(approx(w_clip[0], w_one[0], 1e-12));
    }

    #[test]
    fn irwls_runs_and_returns_finite_weighted_data() {
        // Synthetic single-annotation data: chi2 = 1 + (N/M)*h2*ld (deterministic).
        let n = 50usize;
        let nbar = 1000.0_f64;
        let m_tot = 500.0_f64;
        let hsq_true = 0.2_f64;
        let ld: Vec<f64> = (0..n).map(|i| 1.0 + (i as f64) * 0.3).collect();
        let nvec: Vec<f64> = vec![nbar; n];
        let wld: Vec<f64> = vec![1.0; n];
        let y: Vec<f64> = (0..n)
            .map(|i| 1.0 + (nbar / m_tot) * hsq_true * ld[i])
            .collect();
        // design: N*ld/Nbar = ld (since N=Nbar), plus intercept col.
        let rows: Vec<Vec<f64>> = (0..n).map(|i| vec![ld[i], 1.0]).collect();
        let x = build_mat_row_major(&rows);
        let out = irwls(
            &x, &y, &ld, &wld, &nvec, m_tot, nbar, 1, true, hsq_true, 1.0,
        )
        .unwrap();
        assert_eq!(out.x.nrows(), n);
        assert_eq!(out.x.ncols(), 2);
        assert_eq!(out.y.len(), n);
        assert!(out.y.iter().all(|v| v.is_finite()));
        let all_finite =
            (0..out.x.nrows()).all(|i| (0..out.x.ncols()).all(|j| out.x[(i, j)].is_finite()));
        assert!(all_finite);
    }
}
