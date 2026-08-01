//! Per-region HDL-L analysis driver — 1:1 port of the `HDL.L` function body
//! (`HDL/R/HDL.L.R` lines 48-1099). One call = one genomic region.
//!
//! Pipeline (all in eigen-space, after the variance eigen-cut):
//! 1. LD-score WLS starting values (module [`crate::wls`])
//! 2. eigen-transform `bstar = Vᵀ·bhat` + retain components explaining
//!    `eigen_cut` variance (module [`crate::reference`])
//! 3. univariate h² MLE for each trait (L-BFGS-B multi-start)
//! 4. conditional genetic-covariance MLE (h₁₂ | ĥ²₁, ĥ²₂)
//! 5. LRT P values + profile-likelihood CI for r_G

use faer::MatRef;

use crate::error::{HdlError, Result};
use crate::likelihood::{GcovContext, UnivContext};
use crate::optimize::{lbfgsb_max, lbfgsb_max_1d};
use crate::reference::eigen_select_num;
use crate::stats::{pchisq_sf, qchisq};
use crate::wls::start_values;

/// Default `lim` floor on eigen-terms (R default `exp(-18)`).
pub const DEFAULT_LIM: f64 = 1.522_997_974_471_263e-8; // = exp(-18)
/// Default eigen-cut (retain components explaining ≥ 99% variance).
pub const DEFAULT_EIGEN_CUT: f64 = 0.99;
/// Default significance level for the likelihood-based CI (95%).
pub const DEFAULT_ALPHA: f64 = 0.05;
/// Default LD-reference sample size (UKB white-British, 335,272).
pub const DEFAULT_NREF: f64 = 335_272.0;

/// Per-region HDL-L result.
#[derive(Debug, Clone)]
pub struct LocusResult {
    pub h11: f64,
    pub h22: f64,
    pub h12: f64,
    pub rg: f64,
    pub rg_lower: f64,
    pub rg_upper: f64,
    pub p_h1: f64,
    pub p_h2: f64,
    pub p_h12: f64,
    pub int_h11: f64,
    pub int_h22: f64,
    pub int_h12: f64,
    pub ll_alt_h1: f64,
    pub ll_null_h1: f64,
    pub ll_alt_h2: f64,
    pub ll_null_h2: f64,
    pub ll_alt_h12: f64,
    pub ll_null_h12: f64,
    pub eigen_use: f64,
    pub n_retained: usize,
    pub converged: bool,
}

/// Pearson correlation on complete (non-NaN, paired) observations — R `cor(...,
/// use="complete.obs")`. Scale-invariant, so `cor(bhat1,bhat2) == cor(Z1,Z2)`.
fn cor_complete(x: &[f64], y: &[f64]) -> f64 {
    let mut n = 0.0f64;
    let mut sx = 0.0;
    let mut sy = 0.0;
    let mut sxx = 0.0;
    let mut syy = 0.0;
    let mut sxy = 0.0;
    for (a, b) in x.iter().zip(y) {
        if a.is_nan() || b.is_nan() {
            continue;
        }
        n += 1.0;
        sx += a;
        sy += b;
        sxx += a * a;
        syy += b * b;
        sxy += a * b;
    }
    if n < 2.0 {
        return f64::NAN;
    }
    let mx = sx / n;
    let my = sy / n;
    let cov = sxy / n - mx * my;
    let vx = sxx / n - mx * mx;
    let vy = syy / n - my * my;
    let denom = (vx * vy).sqrt();
    if denom < 1e-300 {
        f64::NAN
    } else {
        cov / denom
    }
}

/// Univariate h² MLE with the exact `HDL.L.R` multi-start (lines 548-595).
///
/// Uses [`UnivContext`] for precomputed function evaluation (lam², lam/N,
/// bstar² are computed once, not per evaluation), with the FD-based L-BFGS-B
/// optimiser that correctly handles the `lim`-floor non-smoothness.
///
/// Returns `(h2, int, ll_alt, ll_null)`.
#[allow(clippy::too_many_arguments)]
fn h2_mle(
    bstar: &[f64],
    lam: &[f64],
    n: f64,
    m: usize,
    nref: f64,
    lim: f64,
    h2_wls_start: f64,
) -> Result<(f64, f64, f64, f64)> {
    let starting_1 = [h2_wls_start, 0.0, 0.5];
    let starting_2 = [1.0_f64, 0.5, 1.5, 0.0];
    let ndeps_values = [1e-5_f64, 1e-8, 1e-16];

    // Build the precomputed context once — avoids recomputing lam², lam/Nref,
    // lam/N, bstar² on every likelihood evaluation across all 36 × 2 starts.
    let ctx = UnivContext::new(lam, bstar, n, m, nref, lim);

    let mut best = f64::NEG_INFINITY;
    let mut out = (f64::NAN, f64::NAN, f64::NAN, f64::NAN);

    for &sv1 in &starting_1 {
        for &sv2 in &starting_2 {
            for &nd in &ndeps_values {
                let alt = lbfgsb_max(
                    |p: &[f64]| ctx.ll(p[0], p[1]),
                    &[sv1, sv2],
                    &[0.0, 0.0],
                    &[1.0, 20.0],
                    &[nd, nd * 100.0],
                )?;
                let null = lbfgsb_max_1d(
                    |int: f64| ctx.ll_null(int),
                    sv2,
                    0.0,
                    20.0,
                    nd * 100.0,
                )?;
                if alt.converged && null.converged && alt.value > best && alt.value > null.value {
                    best = alt.value;
                    out = (alt.par[0], alt.par[1], alt.value, null.value);
                }
            }
        }
    }
    if !out.0.is_finite() {
        return Err(HdlError::NoConverge(
            "heritability optimisation did not converge".into(),
        ));
    }
    Ok(out)
}

/// Conditional genetic-covariance MLE with the exact `HDL.L.R` multi-start
/// (lines 718-790). Uses [`GcovContext`] for precomputed function evaluation.
///
/// Returns `(h12, int, ll_alt, ll_null)`.
#[allow(clippy::too_many_arguments)]
fn gcov_mle(
    bstar1: &[f64],
    bstar2: &[f64],
    lam: &[f64],
    h11: &[f64; 2],
    h22: &[f64; 2],
    n1: f64,
    n2: f64,
    n0: f64,
    nref: f64,
    m: usize,
    lim: f64,
    h12_wls_start: f64,
    rho12: f64,
) -> Result<(f64, f64, f64, f64)> {
    let bound = (h11[0] * h22[0]).sqrt();
    let starting_1 = [h12_wls_start, 0.0, -bound * 0.5, bound * 0.5];
    let starting_2 = [rho12, 1.0, 0.0];
    let ndeps_values = [1e-5_f64, 1e-8, 1e-16];

    // Build the precomputed context once — lam11_k and lam22_k (functions of
    // the fixed per-trait MLEs) are the dominant per-component cost; precomputing
    // them eliminates ~2/3 of the arithmetic from every ll_gcov evaluation.
    let ctx = GcovContext::new(
        h11, h22, m, n1, n2, n0, nref, lam, lam, bstar1, bstar2, lim,
    );

    let mut best = f64::NEG_INFINITY;
    let mut out = (f64::NAN, f64::NAN, f64::NAN, f64::NAN);

    for &sv1 in &starting_1 {
        for &sv2 in &starting_2 {
            for &nd in &ndeps_values {
                let alt = lbfgsb_max(
                    |p: &[f64]| ctx.ll(p[0], p[1]),
                    &[sv1, sv2],
                    &[-bound, -20.0],
                    &[bound, 20.0],
                    &[nd, nd * 100.0],
                )?;
                let null = lbfgsb_max_1d(
                    |int: f64| ctx.ll(0.0, int),
                    sv2,
                    -20.0,
                    20.0,
                    nd * 100.0,
                )?;
                if alt.converged && null.converged && alt.value > best && alt.value > null.value {
                    best = alt.value;
                    out = (alt.par[0], alt.par[1], alt.value, null.value);
                }
            }
        }
    }
    if !out.0.is_finite() {
        return Err(HdlError::NoConverge(
            "genetic covariance optimisation did not converge".into(),
        ));
    }
    Ok(out)
}

/// Run the full HDL-L pipeline for one region.
///
/// - `bhat1`, `bhat2`: per-SNP `Z/√N`, aligned to the reference (length `M`)
/// - `lam`: eigenvalues of the LD correlation matrix R, **descending** (length `M`)
/// - `v`: eigenvectors of R as columns (`M × M`)
/// - `ldsc`: per-SNP LD scores (length `M`)
/// - `n1`, `n2`: per-trait sample sizes; `n0`: sample overlap (0 if independent)
#[allow(clippy::too_many_arguments)]
pub fn run_locus(
    bhat1: &[f64],
    bhat2: &[f64],
    lam: &[f64],
    v: MatRef<f64>,
    ldsc: &[f64],
    n1: f64,
    n2: f64,
    n0: f64,
    nref: f64,
    eigen_cut: f64,
    lim: f64,
    alpha: f64,
) -> Result<LocusResult> {
    let m_tot = bhat1.len(); // M.ref — total SNPs (the likelihood denominator)
    // ---- 1. starting values (LD-score regression on per-SNP quantities) ----
    let rho12 = cor_complete(bhat1, bhat2);
    let sv = start_values(bhat1, bhat2, ldsc, n1, n2, n0, rho12);

    // ---- 2. eigen-transform bstar = Vᵀ·bhat ----
    let m = lam.len();
    if v.nrows() != m || v.ncols() != m {
        return Err(HdlError::Numeric(format!(
            "run_locus: V is {}×{} but expected {m}×{m}",
            v.nrows(),
            v.ncols()
        )));
    }
    let bstar1: Vec<f64> = (0..m)
        .map(|k| (0..m).map(|i| v[(i, k)] * bhat1[i]).sum())
        .collect();
    let bstar2: Vec<f64> = (0..m)
        .map(|k| (0..m).map(|i| v[(i, k)] * bhat2[i]).sum())
        .collect();

    // ---- 3. eigen-cut: retain components explaining ≥ eigen_cut variance ----
    let n_keep = eigen_select_num(lam, eigen_cut);
    let lam_k: Vec<f64> = lam[..n_keep].to_vec();
    let bstar1_k: Vec<f64> = bstar1[..n_keep].to_vec();
    let bstar2_k: Vec<f64> = bstar2[..n_keep].to_vec();

    // ---- 4. univariate h² MLE for each trait ----
    let (h11, int_h11, ll_alt_1, ll_null_1) =
        h2_mle(&bstar1_k, &lam_k, n1, m_tot, nref, lim, sv.h11_wls[0])?;
    let (h22, int_h22, ll_alt_2, ll_null_2) =
        h2_mle(&bstar2_k, &lam_k, n2, m_tot, nref, lim, sv.h22_wls[0])?;

    let p_h1 = pchisq_sf(-2.0 * (ll_null_1 - ll_alt_1), 1.0);
    let p_h2 = pchisq_sf(-2.0 * (ll_null_2 - ll_alt_2), 1.0);

    // ---- 5. genetic covariance (skip if either h² ≤ 0) ----
    if h11 <= 0.0 || h22 <= 0.0 {
        return Ok(LocusResult {
            h11,
            h22,
            h12: f64::NAN,
            rg: f64::NAN,
            rg_lower: f64::NAN,
            rg_upper: f64::NAN,
            p_h1,
            p_h2,
            p_h12: f64::NAN,
            int_h11,
            int_h22,
            int_h12: f64::NAN,
            ll_alt_h1: ll_alt_1,
            ll_null_h1: ll_null_1,
            ll_alt_h2: ll_alt_2,
            ll_null_h2: ll_null_2,
            ll_alt_h12: f64::NAN,
            ll_null_h12: f64::NAN,
            eigen_use: eigen_cut,
            n_retained: n_keep,
            converged: false,
        });
    }

    let (h12, int_h12, ll_alt_12, ll_null_12) = gcov_mle(
        &bstar1_k,
        &bstar2_k,
        &lam_k,
        &[h11, int_h11],
        &[h22, int_h22],
        n1,
        n2,
        n0,
        nref,
        m_tot,
        lim,
        sv.h12_wls[0],
        rho12,
    )?;
    let p_h12 = pchisq_sf(-2.0 * (ll_null_12 - ll_alt_12), 1.0);

    // ---- 6. profile-likelihood CI for r_G (HDL.L.R lines 850-880) ----
    let denom = (h11 * h22).sqrt();
    let c_cutoff = (-qchisq(1.0 - alpha, 1) / 2.0).exp();
    let n_grid = 10_000usize;
    // Build the gcov context once — lam11/lam22 are constant across all 10K
    // grid points; only h12 varies. This eliminates ~2/3 of the per-point work.
    let ctx_ci = GcovContext::new(
        &[h11, int_h11],
        &[h22, int_h22],
        m_tot,
        n1,
        n2,
        n0,
        nref,
        &lam_k,
        &lam_k,
        &bstar1_k,
        &bstar2_k,
        lim,
    );
    let mut h12_val = vec![0.0f64; n_grid];
    let mut ll_vals = vec![f64::NEG_INFINITY; n_grid];
    let mut best_idx = 0usize;
    let mut best_ll = f64::NEG_INFINITY;
    for i in 0..n_grid {
        let hv = -denom + (2.0 * denom) * (i as f64) / ((n_grid - 1) as f64);
        h12_val[i] = hv;
        let lv = ctx_ci.ll(hv, int_h12);
        ll_vals[i] = lv;
        if lv > best_ll {
            best_ll = lv;
            best_idx = i;
        }
    }
    let h12_lci = h12_val[best_idx]; // grid-argmax (R reports rg from this)
    let rg = if denom > 0.0 {
        h12_lci / denom
    } else {
        f64::NAN
    };

    // CI = {h12 : exp(ll - max ll) > c}
    let (mut rg_lower, mut rg_upper) = (f64::NAN, f64::NAN);
    for i in 0..n_grid {
        let likelihood = (ll_vals[i] - best_ll).exp();
        if likelihood > c_cutoff {
            let r = h12_val[i] / denom;
            if rg_lower.is_nan() || r < rg_lower {
                rg_lower = r;
            }
            if rg_upper.is_nan() || r > rg_upper {
                rg_upper = r;
            }
        }
    }
    let rg_lower = rg_lower.max(-1.0);
    let rg_upper = rg_upper.min(1.0);

    Ok(LocusResult {
        h11,
        h22,
        h12,
        rg,
        rg_lower,
        rg_upper,
        p_h1,
        p_h2,
        p_h12,
        int_h11,
        int_h22,
        int_h12,
        ll_alt_h1: ll_alt_1,
        ll_null_h1: ll_null_1,
        ll_alt_h2: ll_alt_2,
        ll_null_h2: ll_null_2,
        ll_alt_h12: ll_alt_12,
        ll_null_h12: ll_null_12,
        eigen_use: eigen_cut,
        n_retained: n_keep,
        converged: true,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use faer::Mat;

    #[test]
    fn run_locus_smoke_synthetic() {
        // Build a small synthetic region: 20 SNPs, identity-ish LD with a mild
        // correlation block. Just checks the pipeline runs end-to-end.
        let m = 20usize;
        // diagonal LD with a 2x2 correlation block → non-trivial eigenvectors
        let mut r = Mat::<f64>::identity(m, m);
        r[(0, 1)] = 0.5;
        r[(1, 0)] = 0.5;
        let (lam, v) = lava::decompose::sym_eigen(r.as_ref()).unwrap();
        let ldsc: Vec<f64> = (0..m).map(|i| 1.0 + 0.5 * (i as f64)).collect();
        let bhat1: Vec<f64> = (0..m).map(|i| 0.01 * ((i as f64) - 5.0)).collect();
        let bhat2: Vec<f64> = (0..m).map(|i| 0.008 * ((i as f64) - 5.0)).collect();

        let res = run_locus(
            &bhat1,
            &bhat2,
            &lam,
            v.as_ref(),
            &ldsc,
            100_000.0,
            120_000.0,
            0.0,
            DEFAULT_NREF,
            DEFAULT_EIGEN_CUT,
            DEFAULT_LIM,
            DEFAULT_ALPHA,
        )
        .expect("run_locus");
        assert!(
            res.h11.is_finite() && res.h11 >= 0.0 && res.h11 <= 1.0,
            "h11={}",
            res.h11
        );
        assert!(
            res.h22.is_finite() && res.h22 >= 0.0 && res.h22 <= 1.0,
            "h22={}",
            res.h22
        );
        assert!(res.p_h1 >= 0.0 && res.p_h1 <= 1.0, "p_h1={}", res.p_h1);
        if res.rg.is_finite() {
            assert!(res.rg >= -1.0 && res.rg <= 1.0, "rg={}", res.rg);
        }
        println!(
            "smoke: h11={:.4} h22={:.4} rg={:?} p_h12={:.4e} (K={})",
            res.h11, res.h22, res.rg, res.p_h12, res.n_retained
        );
    }
}
