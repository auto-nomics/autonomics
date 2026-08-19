//! LD Score Regression estimators — a faithful port of `ldscore/regressions.py`:
//! the shared `LD_Score_Regression` pipeline, plus [`Hsq`], [`Gencov`], and
//! [`RG`], with liability-scale conversions and the χ²/p normal transforms.
//!
//! Shape convention matches Python: response and per-SNP weights are `(n,)`,
//! design is `n × n_annot` (an intercept column is appended for the
//! free-intercept case), `M` is length `n_annot`. The point estimates reported
//! are the **whole-data** estimates (`jknife.est`); standard errors come from
//! the block-jackknife pseudovalues.
//!
//! This module is the comprehensive port; the DataFrame-based
//! [`crate::hsq::estimate_h2`] is numerically equivalent for the non-twostep
//! Hsq case and remains the high-level entry point for that path.

use faer::Mat;

use crate::irwls::{gencov_weights, hsq_weights, irwls_jackknife};
use crate::jackknife::{JackknifeResult, jackknife_fast, ratio_jackknife};
use crate::linalg::build_mat_col_major;
use crate::stats::{chi2_sf_1, norm_isf, norm_pdf};
use crate::{LdscError, Result};

// ---------------------------------------------------------------------------
// Small helpers porting regressions.py utilities
// ---------------------------------------------------------------------------

/// `p_z_norm(est, se)` → (P, Z). Z = est/se (∞ if se = 0); P = χ².sf(Z², 1).
pub fn p_z_norm(est: f64, se: f64) -> (f64, f64) {
    if se == 0.0 || !est.is_finite() {
        return (0.0, f64::INFINITY);
    }
    let z = est / se;
    if !z.is_finite() {
        return (0.0, f64::INFINITY);
    }
    (chi2_sf_1(z * z), z)
}

/// `update_separators(s, ii)` — map masked-space jackknife separators back to
/// unmasked indices. `ii` is the boolean mask; `s` are separators in the masked
/// space (length `n_masked + 1`); returns separators in the full space.
pub fn update_separators(s: &[usize], ii: &[bool]) -> Vec<usize> {
    let maplist: Vec<usize> = (0..ii.len()).filter(|&i| ii[i]).collect();
    let mut t = Vec::with_capacity(s.len());
    t.push(0);
    if s.len() >= 2 {
        for &si in &s[1..s.len() - 1] {
            t.push(maplist[si]);
        }
    }
    t.push(ii.len());
    t
}

/// `h2_obs_to_liab(h2, P, K)` — observed→liability-scale heritability.
pub fn h2_obs_to_liab(h2: f64, p: f64, k: f64) -> Result<f64> {
    if p.is_nan() && k.is_nan() {
        return Ok(h2);
    }
    if k <= 0.0 || k >= 1.0 {
        return Err(LdscError::InvalidInput(
            "K must be in the range (0,1)".into(),
        ));
    }
    if p <= 0.0 || p >= 1.0 {
        return Err(LdscError::InvalidInput(
            "P must be in the range (0,1)".into(),
        ));
    }
    let thresh = norm_isf(k);
    let pdf = norm_pdf(thresh);
    let conv = k * k * (1.0 - k).powi(2) / (p * (1.0 - p) * pdf * pdf);
    Ok(h2 * conv)
}

/// `gencov_obs_to_liab(gencov, P1, P2, K1, K2)` — observed→liability gencov.
/// `None` prevalence (quantitative trait) ⇒ conversion factor 1 on that side.
pub fn gencov_obs_to_liab(
    gencov: f64,
    p1: Option<f64>,
    p2: Option<f64>,
    k1: Option<f64>,
    k2: Option<f64>,
) -> Result<f64> {
    let mut c1 = 1.0;
    let mut c2 = 1.0;
    if let (Some(p1), Some(k1)) = (p1, k1) {
        c1 = h2_obs_to_liab(1.0, p1, k1)?.sqrt();
    }
    if let (Some(p2), Some(k2)) = (p2, k2) {
        c2 = h2_obs_to_liab(1.0, p2, k2)?.sqrt();
    }
    Ok(gencov * c1 * c2)
}

/// numpy-style median (averages the two middle values for even length).
fn median(v: &mut [f64]) -> f64 {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let n = v.len();
    if n % 2 == 1 {
        v[n / 2]
    } else if n == 0 {
        f64::NAN
    } else {
        (v[n / 2 - 1] + v[n / 2]) / 2.0
    }
}

/// `aggregate(y, x, N, M, intercept)` — moment-based h²/gencov seed.
fn aggregate(y: &[f64], x: &[f64], n: &[f64], m_tot: f64, intercept: f64) -> f64 {
    let mean_y = y.iter().sum::<f64>() / y.len() as f64;
    let mean_xn: f64 = x.iter().zip(n.iter()).map(|(xj, nj)| xj * nj).sum::<f64>() / x.len() as f64;
    m_tot * (mean_y - intercept) / mean_xn
}

// ---------------------------------------------------------------------------
// LD_Score_Regression
// ---------------------------------------------------------------------------

/// Output of the shared LD-Score-Regression pipeline (port of the attributes
/// computed in `LD_Score_Regression.__init__`).
#[derive(Debug, Clone)]
pub struct LdScoreRegression {
    pub n_annot: usize,
    pub n_blocks: usize,
    pub constrain_intercept: bool,
    /// Intercept point estimate (`None` if constrained → use the fixed value).
    pub intercept: Option<f64>,
    pub intercept_se: Option<f64>,
    /// Per-annotation coefficients βₖ = estₖ / N̄ (length `n_annot`).
    pub coef: Vec<f64>,
    pub coef_cov: Vec<f64>, // n_annot² row-major
    pub coef_se: Vec<f64>,
    /// Per-annotation h²/gencov `Mₖ·βₖ`.
    pub cat: Vec<f64>,
    pub cat_cov: Vec<f64>,
    pub cat_se: Vec<f64>,
    /// Total h²/gencov `Σ cat`.
    pub tot: f64,
    pub tot_cov: f64,
    pub tot_se: f64,
    /// Per-annotation proportion of tot.
    pub prop: Vec<f64>,
    pub prop_cov: Vec<f64>,
    pub prop_se: Vec<f64>,
    /// Enrichment and proportion of SNPs.
    pub enrichment: Vec<f64>,
    pub m_prop: Vec<f64>,
    /// Block-jackknife delete values for total (`n_blocks`) and partitioned
    /// (`n_blocks × n_annot`).
    pub tot_delete_values: Vec<f64>,
    pub part_delete_values: Vec<Vec<f64>>,
    pub intercept_delete_values: Option<Vec<f64>>,
}

/// Run the shared pipeline. `weight_fn(value, intercept)` returns per-SNP
/// inverse-CVF weights for the current h² (Hsq) or ρg (Gencov) value and
/// intercept. `null_intercept` is 1 for Hsq, 0 for Gencov.
#[allow(clippy::too_many_arguments)]
fn run_ld_score_regression<F>(
    y: &[f64],
    x_ld: &Mat<f64>,
    w_ld: &[f64],
    n_samples: &[f64],
    m: &[f64],
    n_blocks: usize,
    intercept: Option<f64>,
    null_intercept: f64,
    twostep: Option<f64>,
    old_weights: bool,
    weight_fn: &F,
) -> Result<LdScoreRegression>
where
    F: Fn(f64, f64) -> Vec<f64>,
{
    let n = y.len();
    if x_ld.nrows() != n || w_ld.len() != n || n_samples.len() != n {
        return Err(LdscError::DimensionMismatch(
            "LD regression: length mismatch".into(),
        ));
    }
    let n_annot = x_ld.ncols();
    let m_tot: f64 = m.iter().sum();
    let nbar: f64 = n_samples.iter().sum::<f64>() / n as f64;

    // x_tot = Σ_k x_ld[:,k]
    let x_tot: Vec<f64> = (0..n)
        .map(|i| (0..n_annot).map(|k| x_ld[(i, k)]).sum())
        .collect();
    let constrain = intercept.is_some();
    let fixed = intercept.unwrap_or(null_intercept);
    let intercept_for_agg = if constrain {
        intercept.unwrap()
    } else {
        null_intercept
    };

    let tot_agg = aggregate(y, &x_tot, n_samples, m_tot, intercept_for_agg);
    let initial_w = weight_fn(tot_agg, intercept_for_agg);

    // design: N·x_ld / Nbar, plus intercept column when free.
    let p = if constrain { n_annot } else { n_annot + 1 };
    let x = Mat::from_fn(n, p, |i, j| {
        if j < n_annot {
            n_samples[i] * x_ld[(i, j)] / nbar
        } else {
            1.0
        }
    });
    let yp: Vec<f64> = if constrain {
        y.iter().map(|yi| yi - fixed).collect()
    } else {
        y.to_vec()
    };

    // per-iteration weight update: coef → weights.
    let update = |coef: &[f64]| -> Vec<f64> {
        let value = m_tot * coef[0] / nbar;
        let ic = if constrain { fixed } else { coef[n_annot] };
        weight_fn(value, ic)
    };

    let jknife: JackknifeResult = match twostep {
        Some(cutoff) => {
            if constrain {
                return Err(LdscError::InvalidInput(
                    "twostep is not compatible with constrain_intercept".into(),
                ));
            }
            if n_annot > 1 {
                return Err(LdscError::InvalidInput(
                    "twostep not compatible with partitioned LD Score yet".into(),
                ));
            }
            let ii: Vec<bool> = y.iter().map(|yi| *yi < cutoff).collect();
            run_twostep(
                &x, &yp, &x_tot, w_ld, n_samples, &initial_w, n_blocks, n_annot, m_tot, nbar, &ii,
                weight_fn,
            )?
        }
        None if old_weights => {
            // one application of the initial weights, no reweighting passes.
            let sqrtw: Vec<f64> = initial_w
                .iter()
                .map(|w| if *w > 0.0 { w.sqrt() } else { 0.0 })
                .collect();
            let xw = Mat::from_fn(n, p, |i, j| x[(i, j)] * sqrtw[i]);
            let yw: Vec<f64> = (0..n).map(|i| yp[i] * sqrtw[i]).collect();
            jackknife_fast(&xw, &yw, n_blocks)?
        }
        None => irwls_jackknife(&x, &yp, n_blocks, &initial_w, update)?,
    };

    compute_fields(&jknife, m, nbar, m_tot, n_annot, constrain, intercept)
}

/// Two-step estimator (port of the `step1_ii` branch + `_combine_twostep_jknives`).
#[allow(clippy::too_many_arguments)]
fn run_twostep<F>(
    x: &Mat<f64>,
    yp: &[f64],
    _x_tot: &[f64],
    _w_ld: &[f64],
    _n_samples: &[f64],
    initial_w: &[f64],
    n_blocks: usize,
    n_annot: usize,
    m_tot: f64,
    nbar: f64,
    ii: &[bool],
    weight_fn: &F,
) -> Result<JackknifeResult>
where
    F: Fn(f64, f64) -> Vec<f64>,
{
    let n = yp.len();
    let idx1: Vec<usize> = (0..n).filter(|&i| ii[i]).collect();
    let n1 = idx1.len();

    // Step-1 design/response (free intercept on the step-1 subset).
    let p = n_annot + 1;
    let x1 = Mat::from_fn(n1, p, |r, j| x[(idx1[r], j)]);
    let yp1: Vec<f64> = idx1.iter().map(|&i| yp[i]).collect();
    let iw1: Vec<f64> = idx1.iter().map(|&i| initial_w[i]).collect();
    let update1 = |coef: &[f64]| -> Vec<f64> {
        let value = m_tot * coef[0] / nbar;
        let ic = coef[n_annot];
        full_gather(weight_fn, value, ic, &idx1)
    };
    let step1 = irwls_jackknife(&x1, &yp1, n_blocks, &iw1, update1)?;
    let step1_int = step1.est[n_annot];

    // Step-2: constrained-intercept regression of (yp - step1_int) on the
    // N-scaled LD columns (no intercept), over ALL SNPs.
    let yp2: Vec<f64> = yp.iter().map(|y| y - step1_int).collect();
    let x2 = Mat::from_fn(n, n_annot, |i, j| x[(i, j)]);
    let update2 = |coef: &[f64]| -> Vec<f64> {
        let value = m_tot * coef[0] / nbar;
        weight_fn(value, step1_int)
    };
    let seps1 = crate::jackknife::separators(n1, n_blocks);
    let seps_unmasked = update_separators(&seps1, ii);
    let step2 = crate::irwls::irwls_jackknife_with_separators(
        &x2,
        &yp2,
        initial_w,
        update2,
        &seps_unmasked,
    )?;

    // c = Σ(initial_w·x2_raw) / Σ(initial_w·x2_raw²)  where x2_raw = N·ld/Nbar col0
    let x2_col0: Vec<f64> = (0..n).map(|i| x2[(i, 0)]).collect();
    let num: f64 = (0..n).map(|i| initial_w[i] * x2_col0[i]).sum();
    let den: f64 = (0..n).map(|i| initial_w[i] * x2_col0[i] * x2_col0[i]).sum();
    let c = num / den;

    combine_twostep(&step1, &step2, step1_int, c, n_annot)
}

fn full_gather<F: Fn(f64, f64) -> Vec<f64>>(
    weight_fn: &F,
    value: f64,
    ic: f64,
    idx1: &[usize],
) -> Vec<f64> {
    let full = weight_fn(value, ic);
    idx1.iter().map(|&i| full[i]).collect()
}

/// Combine the step-1 (free-intercept) and step-2 (constrained) jackknives.
/// Port of `_combine_twostep_jknives`.
fn combine_twostep(
    step1: &JackknifeResult,
    step2: &JackknifeResult,
    step1_int: f64,
    c: f64,
    n_annot: usize,
) -> Result<JackknifeResult> {
    let nb = step1.delete_values.len();
    // est = [step2.est[0:n_annot], step1_int]
    let mut est = vec![0.0; n_annot + 1];
    est[..n_annot].copy_from_slice(&step2.est[..n_annot]);
    est[n_annot] = step1_int;

    let mut delete_values = vec![vec![0.0; n_annot + 1]; nb];
    for j in 0..nb {
        delete_values[j][n_annot] = step1.delete_values[j][n_annot];
        for a in 0..n_annot {
            delete_values[j][a] =
                step2.delete_values[j][a] - c * (step1.delete_values[j][n_annot] - step1_int);
        }
    }
    // pseudovalues, then jknife stats
    let mut pseudo = vec![vec![0.0; n_annot + 1]; nb];
    for j in 0..nb {
        for a in 0..(n_annot + 1) {
            pseudo[j][a] = (nb as f64) * est[a] - ((nb - 1) as f64) * delete_values[j][a];
        }
    }
    let p = n_annot + 1;
    let mut jknife_est = vec![0.0; p];
    for a in 0..p {
        for j in 0..nb {
            jknife_est[a] += pseudo[j][a];
        }
        jknife_est[a] /= nb as f64;
    }
    let mut cov = vec![0.0; p * p];
    if nb > 1 {
        for a in 0..p {
            for b in 0..p {
                let s: f64 = (0..nb)
                    .map(|j| (pseudo[j][a] - jknife_est[a]) * (pseudo[j][b] - jknife_est[b]))
                    .sum();
                cov[a * p + b] = s / ((nb - 1) as f64 * nb as f64);
            }
        }
    }
    let cov_mat = build_mat_col_major(p, p, &cov);
    let se: Vec<f64> = (0..p).map(|a| cov[a * p + a].max(0.0).sqrt()).collect();
    Ok(JackknifeResult {
        est,
        jknife_est,
        delete_values,
        cov: cov_mat,
        se,
    })
}

/// Compute coef/cat/tot/prop/enrichment/intercept/delete-values from the
/// jackknife result. Port of `_coef/_cat/_tot/_prop/_enrichment/_intercept`.
#[allow(clippy::too_many_arguments)]
fn compute_fields(
    jk: &JackknifeResult,
    m: &[f64],
    nbar: f64,
    m_tot: f64,
    n_annot: usize,
    constrain: bool,
    intercept: Option<f64>,
) -> Result<LdScoreRegression> {
    let nb = jk.delete_values.len();
    // coef, coef_cov
    let mut coef = vec![0.0; n_annot];
    let mut coef_cov = vec![0.0; n_annot * n_annot];
    for a in 0..n_annot {
        coef[a] = jk.est[a] / nbar;
        for b in 0..n_annot {
            coef_cov[a * n_annot + b] = jk.cov[(a, b)] / (nbar * nbar);
        }
    }
    let coef_se: Vec<f64> = (0..n_annot)
        .map(|a| coef_cov[a * n_annot + a].max(0.0).sqrt())
        .collect();

    // cat = M * coef (elementwise); cat_cov = (Mᵀ M) ⊙ coef_cov
    let cat: Vec<f64> = (0..n_annot).map(|a| m[a] * coef[a]).collect();
    let mut cat_cov = vec![0.0; n_annot * n_annot];
    for a in 0..n_annot {
        for b in 0..n_annot {
            cat_cov[a * n_annot + b] = m[a] * m[b] * coef_cov[a * n_annot + b];
        }
    }
    let cat_se: Vec<f64> = (0..n_annot)
        .map(|a| cat_cov[a * n_annot + a].max(0.0).sqrt())
        .collect();

    // tot
    let tot: f64 = cat.iter().sum();
    let tot_cov: f64 = cat_cov.iter().sum();
    let tot_se = tot_cov.max(0.0).sqrt();

    // prop via RatioJackknife
    let mut numer_delete = vec![vec![0.0; n_annot]; nb];
    for j in 0..nb {
        for a in 0..n_annot {
            numer_delete[j][a] = m[a] * jk.delete_values[j][a] / nbar;
        }
    }
    let denom_val: Vec<f64> = (0..nb)
        .map(|j| {
            let s: f64 = numer_delete[j].iter().sum();
            s
        })
        .collect();
    let denom_delete: Vec<Vec<f64>> = (0..nb).map(|j| vec![denom_val[j]; n_annot]).collect();
    let prop_est: Vec<f64> = (0..n_annot).map(|a| cat[a] / tot).collect();
    let rj = ratio_jackknife(&prop_est, &numer_delete, &denom_delete)?;
    let prop = rj.est;
    let prop_cov = rj.cov;
    let prop_se = rj.se;

    // enrichment, M_prop
    let m_prop: Vec<f64> = (0..n_annot).map(|a| m[a] / m_tot).collect();
    let enrichment: Vec<f64> = (0..n_annot)
        .map(|a| (cat[a] / m[a]) / (tot / m_tot))
        .collect();

    // delete values for tot / part
    let tot_delete_values: Vec<f64> = (0..nb)
        .map(|j| {
            (0..n_annot)
                .map(|a| m[a] * jk.delete_values[j][a] / nbar)
                .sum()
        })
        .collect();
    let part_delete_values: Vec<Vec<f64>> = (0..nb)
        .map(|j| {
            (0..n_annot)
                .map(|a| jk.delete_values[j][a] / nbar)
                .collect()
        })
        .collect();

    let (intercept_est, intercept_se, intercept_delete_values) = if constrain {
        (intercept, None, None)
    } else {
        let ie = jk.est[n_annot];
        let ise = jk.cov[(n_annot, n_annot)].max(0.0).sqrt();
        let idv: Vec<f64> = (0..nb).map(|j| jk.delete_values[j][n_annot]).collect();
        (Some(ie), Some(ise), Some(idv))
    };

    Ok(LdScoreRegression {
        n_annot,
        n_blocks: nb,
        constrain_intercept: constrain,
        intercept: intercept_est,
        intercept_se,
        coef,
        coef_cov,
        coef_se,
        cat,
        cat_cov,
        cat_se,
        tot,
        tot_cov,
        tot_se,
        prop,
        prop_cov,
        prop_se,
        enrichment,
        m_prop,
        tot_delete_values,
        part_delete_values,
        intercept_delete_values,
    })
}

// ---------------------------------------------------------------------------
// Hsq
// ---------------------------------------------------------------------------

/// SNP-heritability regression (`Hsq`). Port of `regressions.py: Hsq`.
pub struct Hsq {
    pub reg: LdScoreRegression,
    pub mean_chisq: f64,
    pub lambda_gc: f64,
    pub ratio: Option<f64>,
    pub ratio_se: Option<f64>,
}

impl Hsq {
    /// `Hsq(chisq, x, w, N, M, n_blocks, intercept, twostep, old_weights)`.
    /// `chisq` is Z² (length n); `x` is `n × n_annot` LD scores; `w` length n;
    /// `N` length n; `M` length n_annot.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        chisq: &[f64],
        x: &Mat<f64>,
        w: &[f64],
        n: &[f64],
        m: &[f64],
        n_blocks: usize,
        intercept: Option<f64>,
        twostep: Option<f64>,
        old_weights: bool,
    ) -> Result<Self> {
        let n_annot = x.ncols();
        let m_tot: f64 = m.iter().sum();
        let ld_tot: Vec<f64> = (0..chisq.len())
            .map(|i| (0..n_annot).map(|k| x[(i, k)]).sum())
            .collect();
        let weight_fn =
            |value: f64, ic: f64| -> Vec<f64> { hsq_weights(&ld_tot, w, n, m_tot, value, ic) };
        let reg = run_ld_score_regression(
            chisq,
            x,
            w,
            n,
            m,
            n_blocks,
            intercept,
            1.0,
            twostep,
            old_weights,
            &weight_fn,
        )?;

        let mean_chisq = chisq.iter().sum::<f64>() / chisq.len() as f64;
        let mut chi_sorted = chisq.to_vec();
        let lambda_gc = median(&mut chi_sorted) / 0.4549;
        let (ratio, ratio_se) = if !reg.constrain_intercept {
            let ic = reg.intercept.unwrap_or(1.0);
            let ise = reg.intercept_se.unwrap_or(f64::NAN);
            if mean_chisq > 1.0 {
                let denom = mean_chisq - 1.0;
                (Some((ic - 1.0) / denom), Some(ise / denom))
            } else {
                (None, None)
            }
        } else {
            (None, None)
        };

        Ok(Hsq {
            reg,
            mean_chisq,
            lambda_gc,
            ratio,
            ratio_se,
        })
    }

    /// `Hsq.weights` — port of the classmethod.
    pub fn weights(
        ld: &[f64],
        w_ld: &[f64],
        n: &[f64],
        m_tot: f64,
        hsq: f64,
        intercept: f64,
    ) -> Vec<f64> {
        hsq_weights(ld, w_ld, n, m_tot, hsq, intercept)
    }

    /// `Hsq.aggregate`.
    pub fn aggregate(
        chisq: &[f64],
        x_tot: &[f64],
        n: &[f64],
        m_tot: f64,
        intercept: Option<f64>,
    ) -> f64 {
        aggregate(chisq, x_tot, n, m_tot, intercept.unwrap_or(1.0))
    }

    /// `Hsq._summarize_chisq` → (mean_chisq, lambda_gc).
    pub fn summarize_chisq(chisq: &[f64]) -> (f64, f64) {
        let mean_chisq = chisq.iter().sum::<f64>() / chisq.len() as f64;
        let mut s = chisq.to_vec();
        (mean_chisq, median(&mut s) / 0.4549)
    }
}

// ---------------------------------------------------------------------------
// Gencov
// ---------------------------------------------------------------------------

/// Cross-trait genetic covariance regression (`Gencov`). Port of `Gencov`.
pub struct Gencov {
    pub reg: LdScoreRegression,
    pub mean_z1z2: f64,
    pub p: f64,
    pub z: f64,
}

impl Gencov {
    /// `Gencov(z1, z2, x, w, N1, N2, M, hsq1, hsq2, intercept_hsq1, intercept_hsq2,
    /// n_blocks, intercept_gencov, twostep)`.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        z1: &[f64],
        z2: &[f64],
        x: &Mat<f64>,
        w: &[f64],
        n1: &[f64],
        n2: &[f64],
        m: &[f64],
        hsq1: f64,
        hsq2: f64,
        intercept_hsq1: f64,
        intercept_hsq2: f64,
        n_blocks: usize,
        intercept_gencov: Option<f64>,
        twostep: Option<f64>,
    ) -> Result<Self> {
        let nn = z1.len();
        let y: Vec<f64> = (0..nn).map(|i| z1[i] * z2[i]).collect();
        let sqrt_n1n2: Vec<f64> = (0..nn).map(|i| (n1[i] * n2[i]).sqrt()).collect();
        let n_annot = x.ncols();
        let m_tot: f64 = m.iter().sum();
        let ld_tot: Vec<f64> = (0..nn)
            .map(|i| (0..n_annot).map(|k| x[(i, k)]).sum())
            .collect();
        let weight_fn = move |value: f64, ic: f64| -> Vec<f64> {
            gencov_weights(
                &ld_tot,
                w,
                n1,
                n2,
                m_tot,
                hsq1,
                hsq2,
                value,
                ic,
                intercept_hsq1,
                intercept_hsq2,
            )
        };
        let reg = run_ld_score_regression(
            &y,
            x,
            w,
            &sqrt_n1n2,
            m,
            n_blocks,
            intercept_gencov,
            0.0,
            twostep,
            false,
            &weight_fn,
        )?;
        let (p, z) = p_z_norm(reg.tot, reg.tot_se);
        let mean_z1z2 = (0..nn).map(|i| z1[i] * z2[i]).sum::<f64>() / nn as f64;
        Ok(Gencov {
            reg,
            mean_z1z2,
            p,
            z,
        })
    }

    /// `Gencov.weights`.
    #[allow(clippy::too_many_arguments)]
    pub fn weights(
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
        gencov_weights(
            ld,
            w_ld,
            n1,
            n2,
            m_tot,
            h1,
            h2,
            rho_g,
            intercept_gencov,
            intercept_hsq1,
            intercept_hsq2,
        )
    }

    /// `Gencov.aggregate`.
    pub fn aggregate(
        z1z2: &[f64],
        x_tot: &[f64],
        n: &[f64],
        m_tot: f64,
        intercept: Option<f64>,
    ) -> f64 {
        aggregate(z1z2, x_tot, n, m_tot, intercept.unwrap_or(0.0))
    }
}

// ---------------------------------------------------------------------------
// RG  (bivariate genetic correlation)
// ---------------------------------------------------------------------------

/// Genetic correlation (`RG`). Port of `RG`.
pub struct RG {
    pub hsq1: Hsq,
    pub hsq2: Hsq,
    pub gencov: Gencov,
    pub negative_hsq: bool,
    /// rg point estimate (`rg_ratio`). `f64::NAN` when out of bounds.
    pub rg_ratio: f64,
    pub rg_se: f64,
    pub rg_jknife: f64,
    pub p: f64,
    pub z: f64,
}

impl RG {
    /// `RG(z1, z2, x, w, N1, N2, M, intercept_hsq1, intercept_hsq2,
    /// intercept_gencov, n_blocks, twostep)`.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        z1: &[f64],
        z2: &[f64],
        x: &Mat<f64>,
        w: &[f64],
        n1: &[f64],
        n2: &[f64],
        m: &[f64],
        intercept_hsq1: Option<f64>,
        intercept_hsq2: Option<f64>,
        intercept_gencov: Option<f64>,
        n_blocks: usize,
        twostep: Option<f64>,
    ) -> Result<Self> {
        let chisq1: Vec<f64> = z1.iter().map(|z| z * z).collect();
        let chisq2: Vec<f64> = z2.iter().map(|z| z * z).collect();
        tracing::info!(
            n_snp = z1.len(),
            n_annot = x.ncols(),
            n_blocks,
            intercept_hsq1 = ?intercept_hsq1,
            intercept_hsq2 = ?intercept_hsq2,
            intercept_gencov = ?intercept_gencov,
            twostep = ?twostep,
            "LDSC RG fitting trait-1 h2"
        );
        let hsq1 = Hsq::new(
            &chisq1,
            x,
            w,
            n1,
            m,
            n_blocks,
            intercept_hsq1,
            twostep,
            false,
        )?;
        tracing::info!(
            h2 = hsq1.reg.tot,
            h2_se = hsq1.reg.tot_se,
            intercept = ?hsq1.reg.intercept,
            mean_chisq = hsq1.mean_chisq,
            "LDSC RG trait-1 h2 fit completed"
        );
        tracing::info!(
            n_snp = z2.len(),
            n_annot = x.ncols(),
            n_blocks,
            intercept_hsq2 = ?intercept_hsq2,
            twostep = ?twostep,
            "LDSC RG fitting trait-2 h2"
        );
        let hsq2 = Hsq::new(
            &chisq2,
            x,
            w,
            n2,
            m,
            n_blocks,
            intercept_hsq2,
            twostep,
            false,
        )?;
        tracing::info!(
            h2 = hsq2.reg.tot,
            h2_se = hsq2.reg.tot_se,
            intercept = ?hsq2.reg.intercept,
            mean_chisq = hsq2.mean_chisq,
            "LDSC RG trait-2 h2 fit completed"
        );
        tracing::info!(
            n_snp = z1.len(),
            n_annot = x.ncols(),
            n_blocks,
            intercept_gencov = ?intercept_gencov,
            h2_1 = hsq1.reg.tot,
            h2_2 = hsq2.reg.tot,
            "LDSC RG fitting genetic covariance"
        );
        let gencov = Gencov::new(
            z1,
            z2,
            x,
            w,
            n1,
            n2,
            m,
            hsq1.reg.tot,
            hsq2.reg.tot,
            hsq1.reg.intercept.unwrap_or(1.0),
            hsq2.reg.intercept.unwrap_or(1.0),
            n_blocks,
            intercept_gencov,
            twostep,
        )?;
        tracing::info!(
            gencov = gencov.reg.tot,
            gencov_se = gencov.reg.tot_se,
            mean_z1z2 = gencov.mean_z1z2,
            "LDSC RG genetic covariance fit completed"
        );

        if hsq1.reg.tot <= 0.0 || hsq2.reg.tot <= 0.0 {
            return Ok(RG {
                hsq1,
                hsq2,
                gencov,
                negative_hsq: true,
                rg_ratio: f64::NAN,
                rg_se: f64::NAN,
                rg_jknife: f64::NAN,
                p: f64::NAN,
                z: f64::NAN,
            });
        }

        let rg_ratio = gencov.reg.tot / (hsq1.reg.tot * hsq2.reg.tot).sqrt();
        let nb = hsq1.reg.tot_delete_values.len();
        let denom_delete: Vec<Vec<f64>> = (0..nb)
            .map(|j| vec![(hsq1.reg.tot_delete_values[j] * hsq2.reg.tot_delete_values[j]).sqrt()])
            .collect();
        let numer_delete: Vec<Vec<f64>> = (0..nb)
            .map(|j| vec![gencov.reg.tot_delete_values[j]])
            .collect();
        let rj = ratio_jackknife(&[rg_ratio], &numer_delete, &denom_delete)?;
        let (p, z) = p_z_norm(rg_ratio, rj.se[0]);
        Ok(RG {
            hsq1,
            hsq2,
            gencov,
            negative_hsq: false,
            rg_ratio,
            rg_se: rj.se[0],
            rg_jknife: rj.jknife_est[0],
            p,
            z,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::linalg::build_mat_row_major;

    fn approx(a: f64, b: f64, rtol: f64) -> bool {
        (a - b).abs() <= rtol * b.abs().max(1.0)
    }

    #[test]
    fn p_z_norm_matches_python() {
        // est=10, se=1 → z=10, p*1e23 ≈ 1.523971
        let (p, z) = p_z_norm(10.0, 1.0);
        assert_eq!(z, 10.0);
        assert!((p * 1e23 - 1.523971).abs() < 1e-3, "p*1e23={}", p * 1e23);
        // se=0 → p=0, z=inf
        let (p, z) = p_z_norm(10.0, 0.0);
        assert_eq!(p, 0.0);
        assert!(z.is_infinite());
    }

    #[test]
    fn update_separators_roundtrip() {
        let ii = [true, true, false, true, true, false, true];
        let n1: usize = ii.iter().filter(|x| **x).count();
        let s: Vec<usize> = (0..=n1).collect();
        let t = update_separators(&s, &ii);
        assert_eq!(t[0], 0);
        assert_eq!(*t.last().unwrap(), ii.len());
        // masked index k maps to the k-th true index
        let maplist: Vec<usize> = (0..ii.len()).filter(|i| ii[*i]).collect();
        for k in 1..s.len() - 1 {
            assert_eq!(t[k], maplist[s[k]]);
        }
    }

    #[test]
    fn h2_obs_to_liab_scz() {
        // balanced study of a 1% phenotype ≈ 0.5519
        let x = h2_obs_to_liab(1.0, 0.5, 0.01).unwrap();
        assert!((x - 0.551907298063).abs() < 1e-5, "x = {x}");
    }

    #[test]
    fn h2_obs_to_liab_bad_data() {
        assert!(h2_obs_to_liab(1.0, 1.0, 0.5).is_err());
        assert!(h2_obs_to_liab(1.0, 0.5, 1.0).is_err());
        assert!(h2_obs_to_liab(1.0, 0.0, 0.5).is_err());
        assert!(h2_obs_to_liab(1.0, 0.5, 0.0).is_err());
    }

    #[test]
    fn gencov_obs_to_liab_cases() {
        // QT (all None) → 1
        assert!((gencov_obs_to_liab(1.0, None, None, None, None).unwrap() - 1.0).abs() < 1e-12);
        let v = gencov_obs_to_liab(1.0, Some(0.5), None, Some(0.01), None).unwrap();
        assert!((v - (0.551907298063f64).sqrt()).abs() < 1e-5, "v={v}");
        let v = gencov_obs_to_liab(1.0, Some(0.5), Some(0.5), Some(0.01), Some(0.01)).unwrap();
        assert!((v - 0.551907298063).abs() < 1e-5, "v={v}");
    }

    fn make_ld(n: usize, seed_base: f64) -> Vec<Vec<f64>> {
        // deterministic pseudo-random |x|+1 (avoids rng)
        (0..n)
            .map(|i| {
                let v = ((i as f64 * 12.9898 + seed_base).sin() * 43758.5453)
                    .fract()
                    .abs()
                    + 1.0;
                vec![v]
            })
            .collect()
    }

    #[test]
    fn hsq_aggregate_half() {
        // chisq=1.5, ld=100, N=1e5, M=1e7 → 0.5
        let chisq = vec![1.5; 10];
        let xtot = vec![100.0; 10];
        let n = vec![1e5; 10];
        let agg = Hsq::aggregate(&chisq, &xtot, &n, 1e7, None);
        assert!((agg - 0.5).abs() < 1e-6);
        let agg = Hsq::aggregate(&chisq, &xtot, &n, 1e7, Some(1.5));
        assert!(agg.abs() < 1e-6);
    }

    #[test]
    fn hsq_weights_value_and_clip() {
        // test_weights: w[0] = 0.5/(1+hsq*N/M)²
        let ld = vec![1.0; 4];
        let wld = vec![1.0; 4];
        let n = vec![9.0; 4];
        let m = 7.0;
        let hsq = 0.5;
        let w = Hsq::weights(&ld, &wld, &n, m, hsq, 1.0);
        let expected = 0.5 / (1.0 + hsq * 9.0 / m).powi(2);
        assert!(approx(w[0], expected, 1e-9));
        // out-of-bounds h2 clipped
        let w_hi = Hsq::weights(&ld, &wld, &n, m, 2.0, 1.0);
        let w_one = Hsq::weights(&ld, &wld, &n, m, 1.0, 1.0);
        for i in 0..4 {
            assert!(approx(w_hi[i], w_one[i], 1e-9));
        }
    }

    #[test]
    fn hsq_summarize_chisq() {
        let chisq: Vec<f64> = (0..100).map(|i| i as f64).collect();
        let (mean, lam) = Hsq::summarize_chisq(&chisq);
        assert!((mean - 49.5).abs() < 1e-9);
        assert!((lam - 108.81512420312156).abs() < 1e-6);
    }

    #[test]
    fn hsq_coef_cat_tot_prop_enrichment() {
        // Test_Coef: recover known partitioned h2.
        let hsq1 = 0.2_f64;
        let hsq2 = 0.7_f64;
        let m = vec![1e7 / 2.0; 2];
        let n = 400usize;
        // build ld (n×2) and chisq deterministically
        let rows: Vec<Vec<f64>> = (0..n)
            .map(|i| {
                let a = ((i as f64 * 12.9898).sin() * 43758.5453).fract().abs() + 1.0;
                let b = ((i as f64 * 78.233).sin() * 43758.5453).fract().abs() + 1.0;
                vec![a, b]
            })
            .collect();
        let ld = build_mat_row_major(&rows);
        let nvec = vec![1e5; n];
        let chisq: Vec<f64> = (0..n)
            .map(|i| 1.0 + 1e5 * (ld[(i, 0)] * hsq1 / m[0] + ld[(i, 1)] * hsq2 / m[1]))
            .collect();
        let wld = vec![1.0; n];
        // constrained intercept (intercept=1) → old_weights not used for n_annot>1,
        // but with constrained intercept we skip twostep. Use old_weights=true as
        // Python does for n_annot>1.
        let h = Hsq::new(&chisq, &ld, &wld, &nvec, &m, 3, Some(1.0), None, true).unwrap();
        let a = [hsq1 / m[0], hsq2 / m[1]];
        for k in 0..2 {
            assert!(
                (h.reg.coef[k] - a[k]).abs() < 1e-6,
                "coef[{k}]={}",
                h.reg.coef[k]
            );
        }
        assert!((h.reg.cat[0] - hsq1).abs() < 1e-6);
        assert!((h.reg.cat[1] - hsq2).abs() < 1e-6);
        assert!((h.reg.tot - (hsq1 + hsq2)).abs() < 1e-6);
        let d = hsq1 + hsq2;
        assert!((h.reg.prop[0] - hsq1 / d).abs() < 1e-6);
        // enrichment = (cat/M)/(tot/M_tot); M equal → cat/(tot/2) = 2*cat/tot
        let exp_enrich = (h.reg.cat[0] / m[0]) / ((hsq1 + hsq2) / 1e7);
        assert!((h.reg.enrichment[0] - exp_enrich).abs() < 1e-6);
    }

    #[test]
    fn gencov_weights_equal_hsq_when_z1_eq_z2() {
        let ld = make_ld(100, 1.0);
        let ldv: Vec<f64> = ld.iter().map(|r| r[0]).collect();
        let wld = make_ld(100, 2.0);
        let wldv: Vec<f64> = wld.iter().map(|r| r[0]).collect();
        let n1: Vec<f64> = make_ld(100, 3.0).into_iter().flatten().collect();
        let m = 10.0;
        let wg = Gencov::weights(&ldv, &wldv, &n1, &n1, m, 0.5, 0.5, 0.5, 1.0, 1.0, 1.0);
        let wh = Hsq::weights(&ldv, &wldv, &n1, m, 0.5, 1.0);
        for i in 0..100 {
            assert!(
                (wg[i] - wh[i]).abs() < 1e-9,
                "i={i} wg={} wh={}",
                wg[i],
                wh[i]
            );
        }
    }

    #[test]
    fn rg_negative_correlation() {
        // z2 = -z1, same N → rg ≈ -1
        let n = 50usize;
        let rows: Vec<Vec<f64>> = (0..n)
            .map(|i| {
                let a = ((i as f64 * 12.9898).sin() * 43758.5453).fract().abs() + 2.0;
                let b = ((i as f64 * 78.233).sin() * 43758.5453).fract().abs() + 2.0;
                vec![a, b]
            })
            .collect();
        let ld = build_mat_row_major(&rows);
        let m = vec![700.0, 222.0];
        let nvec = vec![9.0; n];
        let z1: Vec<f64> = (0..n).map(|i| (ld[(i, 0)] + ld[(i, 1)]) * 10.0).collect();
        let z2: Vec<f64> = z1.iter().map(|z| -z).collect();
        let wld = make_ld(n, 5.0);
        let wldv: Vec<f64> = wld.into_iter().flatten().collect();
        let rg = RG::new(
            &z1,
            &z2,
            &ld,
            &wldv,
            &nvec,
            &nvec,
            &m,
            Some(1.0),
            Some(1.0),
            Some(0.0),
            20,
            None,
        )
        .unwrap();
        assert!((rg.rg_ratio + 1.0).abs() < 0.01, "rg_ratio={}", rg.rg_ratio);
    }

    #[test]
    fn rg_negative_h2_is_na() {
        // Mirrors Python Test_RG_Bad: ld=arange+0.1, z1=10/ld, M=-700 → h2<0.
        let n = 50usize;
        let rows: Vec<Vec<f64>> = (0..n).map(|i| vec![i as f64 + 0.1]).collect();
        let ld = build_mat_row_major(&rows);
        let z1: Vec<f64> = (0..n).map(|i| 10.0 / (i as f64 + 0.1)).collect();
        let z2: Vec<f64> = z1.iter().map(|z| -z).collect();
        let m = vec![-700.0];
        let nvec = vec![9.0; n];
        let wld = vec![1.0; n];
        let rg = RG::new(
            &z1,
            &z2,
            &ld,
            &wld,
            &nvec,
            &nvec,
            &m,
            Some(1.0),
            Some(1.0),
            Some(0.0),
            20,
            None,
        )
        .unwrap();
        assert!(rg.negative_hsq);
        assert!(rg.rg_ratio.is_nan());
    }
}
