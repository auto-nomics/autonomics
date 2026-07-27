//! Analysis driver functions — faithful port of `R/analysis_functions.R`.
//!
//! `run_univ`, `run_bivar`, `run_univ_bivar`, `run_multireg`, `run_pcor`, plus
//! the shared `estimate_params` / `filter_params` / `cap` helpers.

use faer::{Mat, MatRef, Side};
use rand::Rng;

use crate::ci::{ci_bivariate, ci_multivariate, ci_pcor};
use crate::locus::Locus;
use crate::pcor::partial_cor;
use crate::stats::{cov2cor, pchisq_sf, pf_sf};
use crate::wishart::{bivariate_integral, integral_p, multivariate_integral};

// ----------------------------- result types -----------------------------

#[derive(Debug, Clone)]
pub struct UnivResult {
    pub phen: String,
    pub var: Option<f64>,
    pub h2_obs: f64,
    pub h2_latent: Option<f64>,
    pub ascertained: Option<bool>,
    pub p: f64,
}

#[derive(Debug, Clone, Default)]
pub struct BivarResult {
    pub phen1: String,
    pub phen2: String,
    pub rho: f64,
    pub rho_lower: f64,
    pub rho_upper: f64,
    pub r2: f64,
    pub r2_lower: f64,
    pub r2_upper: f64,
    pub p: f64,
}

#[derive(Debug, Clone, Default)]
pub struct MultiRegRow {
    pub predictors: String,
    pub outcome: String,
    pub gamma: f64,
    pub gamma_lower: f64,
    pub gamma_upper: f64,
    pub r2: f64,
    pub r2_lower: f64,
    pub r2_upper: f64,
    pub p: f64,
}

#[derive(Debug, Clone, Default)]
pub struct PcorResult {
    pub phen1: String,
    pub phen2: String,
    pub z: String,
    pub r2_phen1_z: f64,
    pub r2_phen2_z: f64,
    pub pcor: f64,
    pub ci_lower: f64,
    pub ci_upper: f64,
    pub p: f64,
}

// ----------------------------- helpers -----------------------------

fn solve_spd(a: MatRef<f64>) -> Option<Mat<f64>> {
    use faer::linalg::solvers::{DenseSolveCore, Llt};
    let llt = Llt::new(a, Side::Lower).ok()?;
    Some(llt.inverse())
}

/// `estimate.params`: MoM coefficients for the chosen phenos (x = all but last,
/// y = last). Returns (coef_std aligned to x, r2).
pub fn estimate_params(omega: MatRef<f64>, idx: &[usize]) -> (Vec<f64>, f64) {
    let m = idx.len();
    let px = m - 1;
    let y = idx[m - 1];
    let x: Vec<usize> = idx[..px].to_vec();
    // omega_xx, omega_xy, omega_yy
    let oxx = Mat::from_fn(px, px, |i, j| omega[(x[i], x[j])]);
    let oxy = Mat::from_fn(1, px, |_, j| omega[(y, x[j])]);
    let inv = match solve_spd(oxx.as_ref()) {
        Some(m) => m,
        None => return (vec![f64::NAN; px], f64::NAN),
    };
    // coef = omega[y,x] · inv  (1×px)
    let mut coef = vec![0.0; px];
    for j in 0..px {
        let mut s = 0.0;
        for k in 0..px {
            s += oxy[(0, k)] * inv[(k, j)];
        }
        coef[j] = s;
    }
    // tau = omega[y,y] - omega[y,x]·inv·omega[x,y]
    let mut quad = 0.0;
    for k in 0..px {
        let mut s = 0.0;
        for l in 0..px {
            s += inv[(k, l)] * omega[(x[l], y)];
        }
        quad += oxy[(0, k)] * s;
    }
    let tau = omega[(y, y)] - quad;
    let oy = omega[(y, y)];
    // standardise
    let coef_std: Vec<f64> = (0..px)
        .map(|j| coef[j] * (omega[(x[j], x[j])] / oy).sqrt())
        .collect();
    let tau_std = tau / oy;
    let r2 = 1.0 - tau_std;
    (coef_std, r2)
}

fn signif6(x: f64) -> f64 {
    if x.is_nan() || x == 0.0 {
        return x;
    }
    let d = 10f64.powi(6 - (x.abs().log10().floor() as i32 + 1));
    (x * d).round() / d
}

fn cap_value(v: f64, lo: f64, hi: f64) -> f64 {
    if v.is_nan() {
        v
    } else if v > hi {
        hi
    } else if v < lo {
        lo
    } else {
        v
    }
}

/// All size-`k` combinations of `items` (in lexicographic order, like R `combn`).
fn combn(items: &[usize], k: usize) -> Vec<Vec<usize>> {
    let n = items.len();
    if k > n {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut combo: Vec<usize> = (0..k).map(|i| items[i]).collect();
    let mut pos: Vec<usize> = (0..k).collect();
    loop {
        out.push(combo.clone());
        // advance
        let mut i = k;
        while i > 0 {
            i -= 1;
            if pos[i] != n - k + i {
                pos[i] += 1;
                combo[i] = items[pos[i]];
                for j in (i + 1)..k {
                    pos[j] = pos[j - 1] + 1;
                    combo[j] = items[pos[j]];
                }
                break;
            }
            if i == 0 {
                return out;
            }
        }
        if pos[0] == n - k {
            // continue until fully advanced past end
        }
        if pos.last() == Some(&n) {
            return out;
        }
    }
}

// ----------------------------- run.univ -----------------------------

/// `univariate.test`: per-phenotype p-value (F-test for continuous, χ² for binary).
pub fn univariate_test(locus: &Locus, phenos: &[String]) -> Vec<f64> {
    let k = locus.k;
    phenos
        .iter()
        .map(|ph| {
            let i = locus.phenos.iter().position(|p| p == ph).unwrap();
            let mut stat = 0.0;
            for r in 0..k {
                stat += locus.delta[(r, i)].powi(2);
            }
            stat = stat / locus.sigma[(i, i)] * locus.nref_scale;
            if locus.binary[i] {
                pchisq_sf(stat, k as f64)
            } else {
                pf_sf(stat / k as f64, k as f64, locus.n[i] - k as f64 - 1.0)
            }
        })
        .collect()
}

/// `run.univ`.
pub fn run_univ(locus: &Locus, phenos: Option<&[String]>, var: bool, cap_estimates: bool) -> Vec<UnivResult> {
    let phenos: Vec<String> = match phenos {
        Some(p) => p.to_vec(),
        None => locus.phenos.clone(),
    };
    let any_binary = locus.binary.iter().any(|&b| b);
    let pvals = univariate_test(locus, &phenos);
    phenos
        .iter()
        .enumerate()
        .map(|(k, ph)| {
            let i = locus.phenos.iter().position(|p| p == ph).unwrap();
            let mut h2_obs = signif6(locus.h2_obs[i]);
            if cap_estimates && h2_obs < 0.0 {
                h2_obs = 0.0;
            }
            let h2_latent = if any_binary {
                let mut v = signif6(locus.h2_latent[i]);
                if cap_estimates && !v.is_nan() && v < 0.0 {
                    v = 0.0;
                }
                Some(v)
            } else {
                None
            };
            let ascertained = if any_binary { Some(locus.ascertained_h2[i]) } else { None };
            UnivResult {
                phen: ph.clone(),
                var: if var { Some(signif6(locus.omega[(i, i)])) } else { None },
                h2_obs,
                h2_latent,
                ascertained,
                p: signif6(pvals[k]),
            }
        })
        .collect()
}

// ----------------------------- run.bivar -----------------------------

/// `run.bivar`.
pub fn run_bivar<R: Rng>(
    locus: &Locus,
    phenos: Option<&[String]>,
    target: Option<&str>,
    adap_thresh: Option<&[f64]>,
    p_values: bool,
    cis: bool,
    param_lim: f64,
    cap_estimates: bool,
    rng: &mut R,
) -> Vec<BivarResult> {
    let phenos: Vec<String> = match phenos {
        Some(p) => p.to_vec(),
        None => locus.phenos.clone(),
    };
    // build pairs
    let pairs: Vec<(String, String)> = match target {
        Some(t) => phenos
            .iter()
            .filter(|p| p.as_str() != t)
            .map(|p| (p.clone(), t.to_string()))
            .collect(),
        None => {
            let mut v = Vec::new();
            for i in 0..phenos.len() {
                for j in (i + 1)..phenos.len() {
                    v.push((phenos[i].clone(), phenos[j].clone()));
                }
            }
            v
        }
    };
    let at: &[f64] = adap_thresh.unwrap_or(&[1e-4, 1e-6]);
    let mut out = Vec::with_capacity(pairs.len());
    for (p1, p2) in pairs {
        let i1 = locus.phenos.iter().position(|p| p == &p1).unwrap();
        let i2 = locus.phenos.iter().position(|p| p == &p2).unwrap();
        // omega/sigma 2×2 sub
        let om = Mat::from_fn(2, 2, |a, b| locus.omega[([i1, i2][a], [i1, i2][b])]);
        let sg = Mat::from_fn(2, 2, |a, b| locus.sigma[([i1, i2][a], [i1, i2][b])]);
        let (coef_std, r2) = estimate_params(locus.omega.as_ref(), &[i1, i2]);
        let rho = signif6(coef_std[0]);
        let mut row = BivarResult {
            phen1: p1.clone(),
            phen2: p2.clone(),
            rho,
            r2: signif6(r2),
            ..Default::default()
        };
        if cis {
            let (rl, ru, r2l, r2u) = ci_bivariate(locus.k, &om, &sg, 10000, rng);
            row.rho_lower = rl;
            row.rho_upper = ru;
            row.r2_lower = r2l;
            row.r2_upper = r2u;
        }
        if p_values {
            let p = integral_p(10000, at, rng, |rng, n| {
                bivariate_integral(locus.k, &om, &sg, n, rng)
            });
            row.p = signif6(p);
        }
        // filter out-of-bounds
        if rho.abs() > param_lim.abs() {
            row.rho = f64::NAN;
            row.rho_lower = f64::NAN;
            row.rho_upper = f64::NAN;
            row.r2 = f64::NAN;
            row.r2_lower = f64::NAN;
            row.r2_upper = f64::NAN;
            row.p = f64::NAN;
        } else if cap_estimates {
            row.rho = cap_value(row.rho, -1.0, 1.0);
            row.r2 = cap_value(row.r2, 0.0, 1.0);
        }
        out.push(row);
    }
    out
}

// ----------------------------- run.univ.bivar -----------------------------

pub struct UnivBivarResult {
    pub univ: Vec<UnivResult>,
    pub bivar: Option<Vec<BivarResult>>,
}

/// `run.univ.bivar`.
pub fn run_univ_bivar<R: Rng>(
    locus: &Locus,
    phenos: Option<&[String]>,
    target: Option<&str>,
    univ_thresh: f64,
    adap_thresh: Option<&[f64]>,
    p_values: bool,
    cis: bool,
    param_lim: f64,
    cap_estimates: bool,
    rng: &mut R,
) -> UnivBivarResult {
    let univ = run_univ(locus, phenos, false, cap_estimates);
    let passing: Vec<String> = univ.iter().filter(|u| u.p < univ_thresh).map(|u| u.phen.clone()).collect();
    let bivar = if passing.len() > 1 {
        let target_ok = target.map(|t| univ.iter().find(|u| u.phen == t).map(|u| u.p < univ_thresh).unwrap_or(false)).unwrap_or(true);
        if target_ok {
            Some(run_bivar(locus, Some(&passing), target, adap_thresh, p_values, cis, param_lim, cap_estimates, rng))
        } else {
            None
        }
    } else {
        None
    };
    UnivBivarResult { univ, bivar }
}

// ----------------------------- run.multireg -----------------------------

/// `run.multireg`: returns a flat list of all (predictor-subset, predictor) rows.
pub fn run_multireg<R: Rng>(
    locus: &Locus,
    target: &str,
    phenos: Option<&[String]>,
    adap_thresh: Option<&[f64]>,
    only_full_model: bool,
    p_values: bool,
    cis: bool,
    param_lim: f64,
    rng: &mut R,
) -> Vec<Vec<MultiRegRow>> {
    let yi = locus.phenos.iter().position(|p| p == target).unwrap();
    let mut phenos: Vec<String> = match phenos {
        Some(p) => p.to_vec(),
        None => locus.phenos.clone(),
    };
    if !phenos.contains(&target.to_string()) {
        phenos.push(target.to_string());
    }
    let x_phenos: Vec<String> = phenos.iter().filter(|p| p.as_str() != target).cloned().collect();
    let x_idx: Vec<usize> = x_phenos
        .iter()
        .map(|p| locus.phenos.iter().position(|q| q == p).unwrap())
        .collect();
    let at: &[f64] = adap_thresh.unwrap_or(&[1e-4, 1e-6]);

    let sizes: Vec<usize> = if only_full_model {
        vec![x_phenos.len()]
    } else {
        (2..=x_phenos.len()).collect()
    };
    let mut result = Vec::new();
    for &sz in &sizes {
        for mod_pred in combn(&x_idx, sz) {
            let mut idx = mod_pred.clone();
            idx.push(yi);
            let om = submatrix(&locus.omega, &idx);
            let sg = submatrix(&locus.sigma, &idx);
            let (gamma_std, r2) = estimate_params(locus.omega.as_ref(), &idx);
            let (gl, gu, r2l, r2u) = if cis {
                ci_multivariate(locus.k, &om, &sg, 10000, rng)
            } else {
                (vec![f64::NAN; sz], vec![f64::NAN; sz], f64::NAN, f64::NAN)
            };
            let pvals = if p_values {
                multivariate_integral(locus.k, &om, &sg, 10000, rng)
            } else {
                vec![f64::NAN; sz]
            };
            let mut rows = Vec::with_capacity(sz);
            for (k, &pi) in mod_pred.iter().enumerate() {
                let mut g = signif6(gamma_std[k]);
                let mut p = signif6(pvals.get(k).copied().unwrap_or(f64::NAN));
                // filter out of bounds (param_lim, multireg uses 1.5)
                if gamma_std[k].abs() > param_lim.abs() {
                    g = f64::NAN;
                    p = f64::NAN;
                }
                let r2_capped = cap_value(signif6(r2), 0.0, 1.0);
                rows.push(MultiRegRow {
                    predictors: locus.phenos[pi].clone(),
                    outcome: target.to_string(),
                    gamma: g,
                    gamma_lower: gl.get(k).copied().unwrap_or(f64::NAN),
                    gamma_upper: gu.get(k).copied().unwrap_or(f64::NAN),
                    r2: r2_capped,
                    r2_lower: r2l,
                    r2_upper: r2u,
                    p,
                });
            }
            result.push(rows);
        }
    }
    result
}

// ----------------------------- run.pcor -----------------------------

/// `run.pcor`.
pub fn run_pcor<R: Rng>(
    locus: &Locus,
    target: (&str, &str),
    phenos: Option<&[String]>,
    adap_thresh: Option<&[f64]>,
    p_values: bool,
    cis: bool,
    max_r2: f64,
    param_lim: f64,
    rng: &mut R,
) -> PcorResult {
    let (t1, t2) = target;
    let mut phenos: Vec<String> = match phenos {
        Some(p) => p.to_vec(),
        None => locus.phenos.clone(),
    };
    for t in [t1, t2] {
        if !phenos.contains(&t.to_string()) {
            phenos.push(t.to_string());
        }
    }
    let xi = locus.phenos.iter().position(|p| p == t1).unwrap();
    let yi = locus.phenos.iter().position(|p| p == t2).unwrap();
    let z_phenos: Vec<String> = phenos.iter().filter(|p| p.as_str() != t1 && p.as_str() != t2).cloned().collect();
    let z_idx: Vec<usize> = z_phenos.iter().map(|p| locus.phenos.iter().position(|q| q == p).unwrap()).collect();

    // r2 of x and y on Z (via bivar if |Z|==1, else multireg full model)
    let r2_xz = r2_target_on_z(locus, xi, &z_idx, rng);
    let r2_yz = r2_target_on_z(locus, yi, &z_idx, rng);

    let mut out = PcorResult {
        phen1: t1.to_string(),
        phen2: t2.to_string(),
        z: z_phenos.join(";"),
        r2_phen1_z: r2_xz,
        r2_phen2_z: r2_yz,
        pcor: f64::NAN,
        ci_lower: f64::NAN,
        ci_upper: f64::NAN,
        p: f64::NAN,
    };
    if !r2_xz.is_nan() && !r2_yz.is_nan() {
        let pcor = partial_cor(locus.omega.as_ref(), xi, yi, &z_idx);
        if let Some(pc) = pcor {
            out.pcor = signif6(pc);
            if cis {
                let (_est, lo, hi) = ci_pcor(locus.k, (xi, yi), &z_idx, &locus.omega, &locus.sigma, 10000, rng);
                out.ci_lower = lo;
                out.ci_upper = hi;
            }
            if p_values && r2_xz < max_r2 && r2_yz < max_r2 {
                // pcov integral p-value — wired via wishart::pcov_integral.
                let at: &[f64] = adap_thresh.unwrap_or(&[1e-4, 1e-6]);
                out.p = signif6(crate::wishart::integral_p(10000, at, rng, |rng, n| {
                    crate::wishart::pcov_integral(locus.k, &locus.omega, &locus.sigma, (xi, yi), &z_idx, n, rng)
                }));
            }
        }
        // filter / cap
        if out.pcor.abs() > param_lim.abs() {
            out.pcor = f64::NAN;
            out.ci_lower = f64::NAN;
            out.ci_upper = f64::NAN;
            out.p = f64::NAN;
        } else {
            out.pcor = cap_value(out.pcor, -1.0, 1.0);
        }
    }
    out
}

/// r2 of `target` on Z (bivariate if |Z|==1, else multireg full model r2[0]).
fn r2_target_on_z<R: Rng>(locus: &Locus, target: usize, z: &[usize], _rng: &mut R) -> f64 {
    if z.is_empty() {
        return f64::NAN;
    }
    if z.len() == 1 {
        let (_coef, r2) = estimate_params(locus.omega.as_ref(), &[z[0], target]);
        r2
    } else {
        let mut idx = z.to_vec();
        idx.push(target);
        let (_coef, r2) = estimate_params(locus.omega.as_ref(), &idx);
        r2
    }
}

fn submatrix(m: &Mat<f64>, idx: &[usize]) -> Mat<f64> {
    let n = idx.len();
    Mat::from_fn(n, n, |i, j| m[(idx[i], idx[j])])
}

// cov2cor re-export for downstream
pub use crate::stats::cov2cor as _cov2cor;
