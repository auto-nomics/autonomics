//! Corrected causal effect — Rust port of `MRlap/R/get_correction.R`.
//!
//! This is MRlap's novel contribution: given the observed IVW-MR effect, the
//! cross-trait LDSC intercept, and the exposure's h², estimate the exposure's
//! genetic architecture (polygenicity π, per-SNP heritability σ²) and solve a
//! closed-form de-biasing formula that simultaneously corrects for sample
//! overlap, weak-instrument bias, and Winner's curse. The SE of the corrected
//! effect (and its covariance with the observed effect) is obtained by a
//! parametric bootstrap; the difference obs vs. corrected is tested with that
//! covariance built into the denominator.
//!
//! The numerical core is a faithful, line-by-line transcription of the R
//! reference. The only deviation is the RNG stream (ChaCha8 instead of R's
//! Mersenne Twister) — the bootstrap estimates the same quantities, so the
//! converged SE/cov agree within Monte-Carlo noise (validated in
//! `tests/cross_validation.rs`).

use crate::error::{MrlapError, Result};
use crate::input::{dnorm, pnorm, z_threshold};
use rand::SeedableRng;
use rand_distr::{Distribution, Normal};

/// Number of independent markers genome-wide — a constant in the R reference
/// (`get_correction.R` line 28: `M=1150000`).
pub const M_CONST: f64 = 1_150_000.0;

// ---------------------------------------------------------------------------
// get_pi  — the function minimised to recover polygenicity π
// ---------------------------------------------------------------------------

/// `get_pi(my_pi, sumbeta2, Tr, n_exp, h2_LDSC, M)` — returns
/// `abs(denominator - sumbeta2)`, where `denominator` is the model-implied
/// Σβ² under a mixture-of-normals genetic architecture (lines 35-44).
#[allow(non_snake_case)]
fn get_pi_loss(
    my_pi: f64,
    sumbeta2: f64,
    Tr: f64,
    n_exp: f64,
    h2_ldsc: f64,
) -> f64 {
    if my_pi <= 0.0 {
        return 1e6;
    }
    let sigma2 = h2_ldsc / (my_pi * M_CONST);
    let n_s2 = n_exp * sigma2; // n_exp * sigma^2

    // A = pnorm(-Tr / sqrt(1 + n_exp*sigma^2))
    let A = pnorm(-Tr / (1.0 + n_s2).sqrt());
    // B = 2*Tr*exp(-Tr^2/(2*(n_exp*sigma^2+1))) / (sqrt(2*pi)*(1+n_exp*sigma^2)^(3/2))
    let B = 2.0 * Tr * (-Tr * Tr / (2.0 * (n_s2 + 1.0))).exp()
        / ((2.0 * std::f64::consts::PI).sqrt() * (1.0 + n_s2).powf(1.5));
    // C terms use pnorm(-Tr) and dnorm(Tr)
    let C_p = pnorm(-Tr);
    let C_d = dnorm(Tr);

    // causal-SNP contribution
    let causal = my_pi
        * (2.0 * (sigma2 + 1.0 / n_exp) * A
            + B * (n_exp * sigma2 * sigma2 + 2.0 * sigma2 + 1.0 / n_exp));
    // null-SNP contribution
    let null = (1.0 - my_pi) * (1.0 / n_exp) * (2.0 * C_p + 2.0 * Tr * C_d);

    let denominator = (causal + null) * M_CONST;
    (denominator - sumbeta2).abs()
}

// ---------------------------------------------------------------------------
// Brent 1-D minimiser — faithful port of R's stats::optimise (optimize.c)
// ---------------------------------------------------------------------------

/// R's `stats::optimise(f, interval, tol)` — finds the minimiser of `f` on
/// `[ax, bx]`. This is a faithful transcription of the Brent (1973) fmin
/// algorithm (golden section + successive parabolic interpolation) used by R's
/// `Brent_combine` (`src/main/optimize.c`) and Numerical Recipes' `brent`, so
/// that π matches the R reference to within `tol`.
fn brent_min<F: FnMut(f64) -> f64>(mut f: F, ax: f64, bx: f64, tol: f64) -> (f64, f64) {
    const CGOLD: f64 = 0.381966011250105;
    const ZEPS: f64 = 1e-10;
    const ITMAX: usize = 100;

    // R's optimise takes an interval, so the third point is the midpoint-ish
    // golden-section guess. NR's brent takes (ax, bx, cx); we set cx = bx and
    // let a/b bracket from ax..bx with bx as the initial best.
    let mut a = if ax < bx { ax } else { bx };
    let mut b = if ax > bx { ax } else { bx };
    let mut x = bx;
    let mut w = bx;
    let mut v = bx;
    let mut fx = f(x);
    let mut fw = fx;
    let mut fv = fx;
    let mut e: f64 = 0.0; // step before last
    let mut d: f64 = 0.0; // last step

    for _ in 0..ITMAX {
        let xm = 0.5 * (a + b);
        let tol1 = tol * x.abs() + ZEPS;
        let tol2 = 2.0 * tol1;
        if (x - xm).abs() <= tol2 - 0.5 * (b - a) {
            return (x, fx);
        }
        let mut use_para = e.abs() > tol1;
        if use_para {
            // parabolic interpolation through (x,fx),(w,fw),(v,fv)
            let r = (x - w) * (fx - fv);
            let mut q = (x - v) * (fx - fw);
            let mut p = (x - v) * q - (x - w) * r;
            q = 2.0 * (q - r);
            if q > 0.0 {
                p = -p;
            }
            q = q.abs();
            let etemp = e;
            e = d;
            if p.abs() < 0.5 * q * etemp.abs() && p > q * (a - x) && p < q * (b - x) {
                d = p / q; // accept parabolic step
                let u = x + d;
                if (u - a) < tol2 || (b - u) < tol2 {
                    d = if xm >= x { tol1 } else { -tol1 };
                }
            } else {
                use_para = false;
            }
        }
        if !use_para {
            // golden-section step
            e = if x >= xm { a - x } else { b - x };
            d = CGOLD * e;
        }
        let u = if d.abs() >= tol1 {
            x + d
        } else {
            x + if xm >= x { tol1 } else { -tol1 }
        };
        let fu = f(u);
        if fu <= fx {
            if u < x {
                b = x;
            } else {
                a = x;
            }
            v = w;
            fv = fw;
            w = x;
            fw = fx;
            x = u;
            fx = fu;
        } else {
            if u < x {
                a = u;
            } else {
                b = u;
            }
            if fu <= fw || w == x {
                v = w;
                fv = fw;
                w = u;
                fw = fu;
            } else if fu <= fv || v == x || v == w {
                v = u;
                fv = fu;
            }
        }
    }
    (x, fx)
}

// ---------------------------------------------------------------------------
// get_geneticArchitecture  — recover (π, σ) from IV effects + h²
// ---------------------------------------------------------------------------

/// `get_geneticArchitecture(theta, n_exp, M, Tr)` (lines 48-73).
///
/// `effects` = IV standardised betas for the exposure; `h2_ldsc` = exposure h².
/// Returns `(pi_x, sigma)`. The R "multi-start" (5 random SPs) is a no-op:
/// `optimise` searches a fixed interval `[1e-7, 0.3]` deterministically, so all
/// starts return the same minimiser — we call it once.
pub fn estimate_genetic_architecture(
    effects: &[f64],
    h2_ldsc: f64,
    n_exp: f64,
    tr: f64,
) -> (f64, f64) {
    let sumbeta2: f64 = effects.iter().map(|e| e * e).sum();
    let (_x, _f) = brent_min(
        |pi| get_pi_loss(pi, sumbeta2, tr, n_exp, h2_ldsc),
        1e-7,
        0.3,
        1e-6,
    );
    let pi_x = _x;
    let sigma = (h2_ldsc / (M_CONST * pi_x)).sqrt();
    (pi_x, sigma)
}

// ---------------------------------------------------------------------------
// get_alpha  — closed-form corrected causal effect
// ---------------------------------------------------------------------------

/// `get_alpha(n_exp, lambdaPrime, pi_x, sigma, alpha_obs, Tr)` (lines 77-93).
#[allow(non_snake_case)]
pub fn corrected_alpha(
    n_exp: f64,
    lambda_prime: f64,
    pi_x: f64,
    sigma: f64,
    alpha_obs: f64,
    tr: f64,
) -> f64 {
    let sigma2 = sigma * sigma;
    let n_s2 = n_exp * sigma2;
    let A = pnorm(-tr / (1.0 + n_s2).sqrt());
    let B = 2.0 * tr * (-tr * tr / (2.0 * (n_s2 + 1.0))).exp()
        / ((2.0 * std::f64::consts::PI).sqrt() * (1.0 + n_s2).powf(1.5));
    let C = pnorm(-tr) + tr * dnorm(tr);

    let a = pi_x
        * (2.0 * (sigma2 + 1.0 / n_exp) * A
            + B * (n_exp * sigma2 * sigma2 + 2.0 * sigma2 + 1.0 / n_exp));
    let b = (1.0 - pi_x) * 2.0 / n_exp * C;
    let d = a + b;

    let numerator =
        alpha_obs * d - (lambda_prime * pi_x * (2.0 * A + B + sigma2 * n_exp * B) + lambda_prime * (1.0 - pi_x) * 2.0 * C);
    let denominator = pi_x * sigma2 * (2.0 * A + B * n_exp * sigma2 + B);
    numerator / denominator
}

// ---------------------------------------------------------------------------
// Parametric bootstrap for SE(corrected) and Cov(obs, corrected)
// ---------------------------------------------------------------------------

/// Inputs to [`correct`]. Matches the argument list of `get_correction`.
#[derive(Clone, Debug)]
pub struct CorrectionInput<'a> {
    /// Exposure standardised IV effects (`IVs$std_beta.exp`) and their SEs.
    pub iv_std_beta_exp: &'a [f64],
    pub iv_std_se_exp: &'a [f64],
    pub lambda: f64,
    pub lambda_se: f64,
    pub h2_ldsc: f64,
    pub h2_ldsc_se: f64,
    pub alpha_obs: f64,
    pub alpha_obs_se: f64,
    pub n_exp: f64,
    /// `n_out` is in the R signature but unused for the correction math.
    pub n_out: f64,
    pub mr_threshold: f64,
}

/// Outputs of [`correct`] — the "MRcorrection" + "GeneticArchitecture" buckets.
#[derive(Clone, Debug)]
pub struct CorrectionResult {
    pub alpha_corrected: f64,
    pub alpha_corrected_se: f64,
    pub cov_obs_corrected: f64,
    pub test_diff: f64,
    pub p_diff: f64,
    pub pi_x: f64,
    pub sigma2_x: f64,
    /// Number of simulations used (rows in `res`).
    pub n_sim: usize,
    /// Whether any bootstrap h² draw was negative (resampled).
    pub neg_h2: bool,
}

/// Defaults matching the R reference (`get_correctedSE` line 102).
const S_BATCH: usize = 1000;
const S_THRESHOLD: f64 = 0.05;
const NUM_GROUPS: usize = 10;
const MAX_MORE: usize = 100;

/// One bootstrap batch (`get_s`, lines 104-133). Returns per-sim `(pi, sigma,
/// alpha_obs, lambda, corrected, neg_h2)`.
#[allow(clippy::type_complexity, clippy::too_many_arguments)]
fn draw_batch(
    rng: &mut rand_chacha::ChaCha8Rng,
    iv_beta: &[f64],
    iv_se: &[f64],
    h2_ldsc: f64,
    h2_ldsc_se: f64,
    lambda: f64,
    lambda_se: f64,
    alpha_obs: f64,
    alpha_obs_se: f64,
    n_exp: f64,
    n_out: f64,
    tr: f64,
    s: usize,
) -> Vec<(f64, f64, f64, f64, f64, bool)> {
    let n_iv = iv_beta.len();
    let lambda_prime_scale = 1.0 / (n_exp * n_out).sqrt();
    // R draws, in order: L (s), E (n_iv*s), H (s), B (s). We reproduce the
    // pairing so a matching RNG stream would give identical results.
    let nrm = Normal::new(0.0, 1.0).unwrap();

    // L ~ N(lambda, lambda_se) / sqrt(n_exp*n_out)
    let l_draws: Vec<f64> = (0..s).map(|_| nrm.sample(rng) * lambda_se + lambda).collect();
    let lambda_prime: Vec<f64> = l_draws.iter().map(|v| v * lambda_prime_scale).collect();

    // E ~ N(effect_i, se_i) per (iv, sim), column-major (sim varies fastest in R matrix)
    let mut e = vec![0.0_f64; n_iv * s];
    // R matrix(data, ncol=s) fills column-major: element (row i, col j) at index i + j*n_iv
    for j in 0..s {
        for i in 0..n_iv {
            e[i + j * n_iv] = nrm.sample(rng) * iv_se[i] + iv_beta[i];
        }
    }

    // H ~ N(h2, h2_se), resample negatives
    let mut neg_h2 = false;
    let mut h: Vec<f64> = (0..s).map(|_| nrm.sample(rng) * h2_ldsc_se + h2_ldsc).collect();
    if h.iter().any(|v| *v < 0.0) {
        neg_h2 = true;
        let mut guard = 0;
        while h.iter().any(|v| *v < 0.0) && guard < 10000 {
            for v in h.iter_mut() {
                if *v < 0.0 {
                    *v = nrm.sample(rng) * h2_ldsc_se + h2_ldsc;
                }
            }
            guard += 1;
        }
    }

    // B ~ N(alpha_obs, alpha_obs_se)
    let b: Vec<f64> = (0..s)
        .map(|_| nrm.sample(rng) * alpha_obs_se + alpha_obs)
        .collect();

    // For each sim j: effects = e[:, j], h2 = h[j] → (pi, sigma); corrected via get_alpha
    let mut out = Vec::with_capacity(s);
    for j in 0..s {
        let effects: Vec<f64> = (0..n_iv).map(|i| e[i + j * n_iv]).collect();
        let (pi_j, sigma_j) = estimate_genetic_architecture(&effects, h[j], n_exp, tr);
        let corrected = corrected_alpha(n_exp, lambda_prime[j], pi_j, sigma_j, b[j], tr);
        out.push((pi_j, sigma_j, b[j], lambda_prime[j], corrected, neg_h2));
    }
    out
}

/// Convergence-check helper: split `rows` into `num_groups` consecutive groups
/// (R's `(row_number()-1) %/% (n()/num_groups)`) and return per-group
/// var(corrected) and cov(corrected, alpha).
///
/// R uses `sqrt(n_exp*n_out)` as the lambda→lambdaPrime scale inside the
/// bootstrap draws (line 108).
fn group_var_cov(rows: &[(f64, f64, f64, f64, f64, bool)], num_groups: usize) -> (Vec<f64>, Vec<f64>) {
    let n = rows.len();
    if n == 0 {
        return (vec![], vec![]);
    }
    let denom = (n as f64) / (num_groups as f64); // n()/num_groups (float)
    let mut groups: Vec<Vec<usize>> = vec![Vec::new(); num_groups];
    for idx in 0..n {
        let g = ((idx as f64) / denom).floor() as usize;
        let g = g.min(num_groups - 1);
        groups[g].push(idx);
    }
    let mut vars = Vec::with_capacity(num_groups);
    let mut covs = Vec::with_capacity(num_groups);
    for g in groups {
        let corrected: Vec<f64> = g.iter().map(|&i| rows[i].4).collect();
        let alpha: Vec<f64> = g.iter().map(|&i| rows[i].2).collect();
        vars.push(sample_var_owned(&corrected));
        covs.push(sample_cov_owned(&corrected, &alpha));
    }
    (vars, covs)
}

/// Sample variance (R's `stats::var`, /(n-1)).
fn sample_var_owned(v: &[f64]) -> f64 {
    if v.len() < 2 {
        return 0.0;
    }
    let mean = v.iter().sum::<f64>() / v.len() as f64;
    let s: f64 = v.iter().map(|x| (x - mean).powi(2)).sum();
    s / (v.len() - 1) as f64
}

/// Sample covariance (R's `stats::cov`, /(n-1)).
fn sample_cov_owned(a: &[f64], b: &[f64]) -> f64 {
    let n = a.len().min(b.len());
    if n < 2 {
        return 0.0;
    }
    let n = n as f64;
    let mx = a.iter().sum::<f64>() / n;
    let my = b.iter().sum::<f64>() / n;
    let s: f64 = a.iter().zip(b.iter()).map(|(x, y)| (x - mx) * (y - my)).sum();
    s / (n - 1.0)
}

/// Full correction — port of `get_correction(...)`.
///
/// `seed` seeds the bootstrap RNG (R uses the global `set.seed`; we make it
/// explicit for reproducibility).
pub fn correct(input: &CorrectionInput<'_>, seed: u64) -> Result<CorrectionResult> {
    if input.iv_std_beta_exp.len() != input.iv_std_se_exp.len() {
        return Err(MrlapError::input(
            "iv_std_beta_exp / iv_std_se_exp length mismatch",
        ));
    }
    let tr = z_threshold(input.mr_threshold);
    let n_exp = input.n_exp;
    let lambda_prime = input.lambda / (n_exp * input.n_out).sqrt();

    // Genetic architecture from the observed IVs.
    let (pi_x, sigma) = estimate_genetic_architecture(
        input.iv_std_beta_exp,
        input.h2_ldsc,
        n_exp,
        tr,
    );
    let alpha_corrected = corrected_alpha(n_exp, lambda_prime, pi_x, sigma, input.alpha_obs, tr);

    // ---- bootstrap SE + cov (get_correctedSE) ----
    let mut rng = rand_chacha::ChaCha8Rng::seed_from_u64(seed);
    let mut rows: Vec<(f64, f64, f64, f64, f64, bool)> = draw_batch(
        &mut rng,
        input.iv_std_beta_exp,
        input.iv_std_se_exp,
        input.h2_ldsc,
        input.h2_ldsc_se,
        input.lambda,
        input.lambda_se,
        input.alpha_obs,
        input.alpha_obs_se,
        n_exp,
        input.n_out,
        tr,
        S_BATCH,
    );
    // tmp_sd_corrected = sd(res$corrected) of the first batch (line 146).
    let tmp_sd_corrected = sample_var_owned(
        &rows.iter().map(|r| r.4).collect::<Vec<_>>(),
    )
    .sqrt();

    // first convergence assessment
    let (vars, covs) = group_var_cov(&rows, NUM_GROUPS);
    let mut needmore = needs_more(&vars, &covs, &rows, input.alpha_obs_se);
    let mut guard = 0;
    while needmore && guard < MAX_MORE {
        let extra = draw_batch(
            &mut rng,
            input.iv_std_beta_exp,
            input.iv_std_se_exp,
            input.h2_ldsc,
            input.h2_ldsc_se,
            input.lambda,
            input.lambda_se,
            input.alpha_obs,
            input.alpha_obs_se,
            n_exp,
            input.n_out,
            tr,
            S_BATCH,
        );
        rows.extend(extra);
        // filter only for the subset diagnostic (res stays unfiltered)
        let filtered: Vec<(f64,f64,f64,f64,f64,bool)> = rows
            .iter()
            .filter(|r| {
                r.4 < alpha_corrected + 10.0 * tmp_sd_corrected
                    && r.4 > alpha_corrected - 10.0 * tmp_sd_corrected
            })
            .cloned()
            .collect();
        let (v2, c2) = group_var_cov(&filtered, NUM_GROUPS);
        let extra_check = input.alpha_obs_se.powi(2)
            + sample_var_owned(&rows.iter().map(|r| r.4).collect::<Vec<_>>())
            - 2.0 * sample_cov_owned(
                &rows.iter().map(|r| r.4).collect::<Vec<_>>(),
                &rows.iter().map(|r| r.2).collect::<Vec<_>>(),
            )
            < 0.0;
        needmore = cv_high(&v2, S_THRESHOLD) || cv_high(&c2, S_THRESHOLD) || extra_check;
        guard += 1;
    }

    let corrected_vals: Vec<f64> = rows.iter().map(|r| r.4).collect();
    let alpha_vals: Vec<f64> = rows.iter().map(|r| r.2).collect();
    let se = sample_var_owned(&corrected_vals).sqrt();
    let cov = sample_cov_owned(&corrected_vals, &alpha_vals);
    let neg_h2 = rows.iter().any(|r| r.5);
    let n_sim = rows.len();

    // test difference (lines 174-176)
    let denom = (input.alpha_obs_se.powi(2) + se.powi(2) - 2.0 * cov).max(0.0).sqrt();
    let test_diff = if denom > 0.0 {
        (input.alpha_obs - alpha_corrected) / denom
    } else {
        f64::NAN
    };
    let p_diff = crate::input::pnorm2_abs(test_diff);

    Ok(CorrectionResult {
        alpha_corrected,
        alpha_corrected_se: se,
        cov_obs_corrected: cov,
        test_diff,
        p_diff,
        pi_x,
        sigma2_x: sigma * sigma,
        n_sim,
        neg_h2,
    })
}

fn cv_high(vals: &[f64], threshold: f64) -> bool {
    let m = vals.iter().sum::<f64>() / vals.len() as f64;
    if m.abs() < 1e-30 {
        return false;
    }
    let sd = sample_var_owned(vals).sqrt();
    sd / m.abs() > threshold
}

fn needs_more(
    vars: &[f64],
    covs: &[f64],
    rows: &[(f64, f64, f64, f64, f64, bool)],
    alpha_obs_se: f64,
) -> bool {
    let v = cv_high(vars, S_THRESHOLD);
    let c = cv_high(covs, S_THRESHOLD);
    let corrected_vals: Vec<f64> = rows.iter().map(|r| r.4).collect();
    let alpha_vals: Vec<f64> = rows.iter().map(|r| r.2).collect();
    let extra = alpha_obs_se.powi(2) + sample_var_owned(&corrected_vals)
        - 2.0 * sample_cov_owned(&corrected_vals, &alpha_vals)
        < 0.0;
    v || c || extra
}
