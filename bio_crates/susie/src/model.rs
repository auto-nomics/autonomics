//! SuSiE model: IBSS initialization, main iteration loop, ELBO computation,
//! convergence checking, and finalization.
//!
//! Faithfully ports `susie_workhorse`, `ibss_initialize`, `ibss_fit`,
//! `single_effect_update`, `compute_residuals.ss`, `update_fitted_values.ss`,
//! `get_objective.default`, `check_convergence.default`, `update_model_variance`,
//! `trim_null_effects`, `ibss_finalize`.

use crate::SusieError;
use crate::cs;
use crate::data::{PriorMethod, RssData};
use crate::ser;
use faer::Mat;

// ─── output fit ──────────────────────────────────────────────────────────────

/// Fitted SuSiE model (the Rust analogue of the `"susie"` R object).
#[derive(Debug, Clone)]
pub struct SusieFit {
    /// L×p posterior inclusion probabilities.
    pub alpha: Vec<Vec<f64>>,
    /// L×p posterior means (conditional on inclusion).
    pub mu: Vec<Vec<f64>>,
    /// L×p posterior second moments (conditional on inclusion).
    pub mu2: Vec<Vec<f64>>,
    /// L prior variances.
    pub v: Vec<f64>,
    /// L model-level log Bayes factors.
    pub lbf: Vec<f64>,
    /// L×p per-variable log Bayes factors.
    pub lbf_variable: Vec<Vec<f64>>,
    /// L KL divergences.
    pub kl: Vec<f64>,
    /// Residual variance.
    pub sigma2: f64,
    /// ELBO at each iteration (after the first).
    pub elbo: Vec<f64>,
    /// Number of IBSS iterations performed.
    pub niter: usize,
    /// Whether the IBSS converged within tolerance.
    pub converged: bool,
    /// Marginal posterior inclusion probabilities (length p).
    pub pip: Vec<f64>,
    /// Credible sets.
    pub sets: crate::cs::CredibleSets,
}

// ─── params ──────────────────────────────────────────────────────────────────

/// Validated parameter bundle.
#[derive(Debug, Clone)]
pub struct SusieParams {
    pub l: usize,
    pub scaled_prior_variance: f64,
    pub residual_variance: f64,
    pub prior_weights: Vec<f64>,
    pub null_weight: f64,
    pub estimate_residual_variance: bool,
    pub estimate_prior_variance: bool,
    pub estimate_prior_method: PriorMethod,
    pub prior_tol: f64,
    pub check_null_threshold: f64,
    pub residual_variance_lowerbound: f64,
    pub residual_variance_upperbound: f64,
    pub coverage: f64,
    pub min_abs_corr: Option<f64>,
    pub median_abs_corr: Option<f64>,
    pub max_iter: usize,
    pub tol: f64,
    pub n: f64,
    pub p: usize,
    pub add_null: bool,
}

// ─── internal model state ────────────────────────────────────────────────────

struct Model {
    l: usize,
    p: usize,
    /// L×p posterior inclusion probabilities.
    alpha: Vec<Vec<f64>>,
    /// L×p posterior means.
    mu: Vec<Vec<f64>>,
    /// L×p posterior second moments.
    mu2: Vec<Vec<f64>>,
    /// L prior variances.
    v: Vec<f64>,
    /// L model-level log Bayes factors.
    lbf: Vec<f64>,
    /// L×p per-variable lbf.
    lbf_variable: Vec<Vec<f64>>,
    /// L KL divergences.
    kl: Vec<f64>,
    /// Residual variance.
    sigma2: f64,
    /// Fitted values: XtX * Σ_l (alpha_l · mu_l), length p.
    xtxr: Vec<f64>,
    converged: bool,
}

impl Model {
    fn new(params: &SusieParams) -> Self {
        let l = params.l;
        let p = params.p;

        // expand_scaled_prior_variance: scalar → repeat L times
        let v = vec![params.scaled_prior_variance; l];

        Self {
            l,
            p,
            alpha: vec![params.prior_weights.clone(); l],
            mu: vec![vec![0.0; p]; l],
            mu2: vec![vec![0.0; p]; l],
            v,
            lbf: vec![f64::NAN; l],
            lbf_variable: vec![vec![f64::NAN; p]; l],
            kl: vec![f64::NAN; l],
            sigma2: params.residual_variance,
            // XtXr = compute_Rv(data, colSums(alpha * mu)) = 0 (mu=0 initially)
            xtxr: vec![0.0; p],
            converged: false,
        }
    }
}

// ─── matrix-vector helpers ───────────────────────────────────────────────────

/// Compute XtX * v (the "compute_Rv" operation).
fn xtx_vec(xtx: &Mat<f64>, v: &[f64]) -> Vec<f64> {
    let p = v.len();
    let mut out = vec![0.0; p];
    for i in 0..p {
        let mut s = 0.0;
        for j in 0..p {
            s += xtx[(i, j)] * v[j];
        }
        out[i] = s;
    }
    out
}

/// Compute row l of (alpha_l * mu_l) %*% XtX — i.e., for each column j:
///   (B_mat[l,:]) %*% XtX[:, j]
/// Used by get_ER2 for per_slot_XB2 = rowSums(compute_BR(data, B) * B).
fn br_row(xtx: &Mat<f64>, b_row: &[f64]) -> Vec<f64> {
    // result[j] = Σ_k b_row[k] * xtx[(k, j)]
    let p = b_row.len();
    let mut out = vec![0.0; p];
    for j in 0..p {
        let mut s = 0.0;
        for k in 0..p {
            s += b_row[k] * xtx[(k, j)];
        }
        out[j] = s;
    }
    out
}

// ─── compute_residuals.ss ────────────────────────────────────────────────────

/// Compute residual betahat for effect l: remove lth effect from fitted values.
///
///   XtXr_without_l = XtXr - compute_Rv(alpha[l,:] * mu[l,:])
///   residuals = Xty - XtXr_without_l
fn compute_residuals(data: &RssData, model: &Model, l: usize) -> (Vec<f64>, Vec<f64>) {
    let p = data.p;
    let am: Vec<f64> = (0..p).map(|j| model.alpha[l][j] * model.mu[l][j]).collect();
    let r_am = xtx_vec(&data.xtx, &am);
    let xtxr_without_l: Vec<f64> = (0..p).map(|j| model.xtxr[j] - r_am[j]).collect();
    let residuals: Vec<f64> = (0..p).map(|j| data.xty[j] - xtxr_without_l[j]).collect();
    (residuals, xtxr_without_l)
}

// ─── get_ER2.ss ──────────────────────────────────────────────────────────────

/// Expected squared residuals:
///   ER2 = yty - 2*betabar'Xty + betabar'XtX*betabar - Σ per_slot_XB2 + Σ per_slot_Eb2
fn get_er2(data: &RssData, model: &Model) -> f64 {
    let p = data.p;
    let l = model.l;

    // B = alpha * mu  (L×p)
    let b: Vec<Vec<f64>> = (0..l)
        .map(|ll| {
            (0..p)
                .map(|j| model.alpha[ll][j] * model.mu[ll][j])
                .collect()
        })
        .collect();

    // betabar = colSums(B)  (Σ_l alpha[l,j]*mu[l,j])
    let betabar: Vec<f64> = (0..p).map(|j| (0..l).map(|ll| b[ll][j]).sum()).collect();

    // betabar' * XtX * betabar
    let r_betabar = xtx_vec(&data.xtx, &betabar);
    let quad: f64 = (0..p).map(|j| betabar[j] * r_betabar[j]).sum();

    // per_slot_XB2[l] = B[l,:] %*% XtX %*% B[l,:]' = rowSums(compute_BR * B)
    let per_slot_xb2: f64 = (0..l)
        .map(|ll| {
            let br = br_row(&data.xtx, &b[ll]);
            (0..p).map(|j| br[j] * b[ll][j]).sum::<f64>()
        })
        .sum();

    // per_slot_Eb2[l] = Σ_j (alpha[l,j] * mu2[l,j]) * predictor_weights[j]
    let per_slot_eb2: f64 = (0..l)
        .map(|ll| {
            (0..p)
                .map(|j| model.alpha[ll][j] * model.mu2[ll][j] * data.predictor_weights[j])
                .sum::<f64>()
        })
        .sum();

    let cross: f64 = (0..p).map(|j| betabar[j] * data.xty[j]).sum();

    data.yty - 2.0 * cross + quad - per_slot_xb2 + per_slot_eb2
}

// ─── Eloglik.ss ──────────────────────────────────────────────────────────────

/// Expected log-likelihood:  -n/2 * log(2π σ²) - 1/(2σ²) * ER2
fn eloglik(data: &RssData, model: &Model) -> f64 {
    let er2 = get_er2(data, model);
    -(data.n / 2.0) * (2.0 * std::f64::consts::PI * model.sigma2).ln()
        - 1.0 / (2.0 * model.sigma2) * er2
}

// ─── ELBO (get_objective.default) ────────────────────────────────────────────

fn get_objective(data: &RssData, model: &Model) -> f64 {
    let el = eloglik(data, model);
    let sum_kl: f64 = model.kl.iter().filter(|k| k.is_finite()).sum();
    el - sum_kl
}

// ─── main fitting loop ───────────────────────────────────────────────────────

/// Run the full IBSS algorithm.
pub fn fit(data: &RssData, params: &mut SusieParams) -> Result<SusieFit, SusieError> {
    let mut model = Model::new(params);

    let max_iter = params.max_iter;
    let mut elbo_history: Vec<f64> = Vec::with_capacity(max_iter + 1);
    elbo_history.push(f64::NEG_INFINITY);

    let mut prev_elbo: f64 = f64::NEG_INFINITY;
    let mut n_iter: usize = 0;

    for iter in 1..=max_iter {
        n_iter = iter;

        // ── ibss_fit: update all L effects ──
        for l in 0..model.l {
            single_effect_update(data, params, &mut model, l);
        }

        // ── compute ELBO ──
        let elbo = get_objective(data, &model);
        if !elbo.is_finite() {
            return Err(SusieError::InfiniteElbo);
        }
        elbo_history.push(elbo);

        // ── check convergence ──
        if iter > 1 {
            let diff = elbo - prev_elbo;
            // Warn if ELBO decreased significantly
            model.converged = diff >= 0.0 && diff < params.tol;
        } else {
            model.converged = false;
        }

        prev_elbo = elbo;

        if model.converged {
            break;
        }

        // ── update residual variance ──
        if params.estimate_residual_variance {
            let er2 = get_er2(data, &model);
            let new_sigma2 = er2 / data.n;
            model.sigma2 = new_sigma2
                .clamp(
                    params.residual_variance_lowerbound,
                    params.residual_variance_upperbound,
                )
                .max(0.0);
        }
    }

    // ── trim null effects ──
    trim_null_effects(params, &mut model);

    // ── finalize ──
    let elbo = elbo_history[1..].to_vec();

    // PIP
    let pip = compute_pip(&model, params.prior_tol);

    // Credible sets
    let sets = cs::compute_cs(&model.alpha, &data.xtx, params);

    Ok(SusieFit {
        alpha: model.alpha,
        mu: model.mu,
        mu2: model.mu2,
        v: model.v,
        lbf: model.lbf,
        lbf_variable: model.lbf_variable,
        kl: model.kl,
        sigma2: model.sigma2,
        elbo,
        niter: n_iter,
        converged: model.converged,
        pip,
        sets,
    })
}

// ─── single_effect_update ────────────────────────────────────────────────────

/// Update one effect: compute residuals → SER → update fitted values.
fn single_effect_update(data: &RssData, params: &SusieParams, model: &mut Model, l: usize) {
    // 1. Compute residuals (remove lth effect)
    let (residuals, fitted_without_l) = compute_residuals(data, model, l);

    // 2. SER statistics
    let stats = ser::compute_ser_stats(&residuals, model.sigma2, &data.predictor_weights);

    // 3. Run SER
    let result = ser::single_effect_regression(
        &stats,
        &params.prior_weights,
        model.v[l],
        Some(&model.alpha[l]),
        Some(&model.mu2[l]),
        params.estimate_prior_variance,
        params.estimate_prior_method,
        params.check_null_threshold,
    );

    // 4. Store results
    model.alpha[l] = result.alpha;
    model.mu[l] = result.mu;
    model.mu2[l] = result.mu2;
    model.v[l] = result.v;
    model.lbf[l] = result.lbf_model;
    model.lbf_variable[l] = result.lbf_variable;
    model.kl[l] = result.kl;

    // 5. Update fitted values: XtXr = fitted_without_l + XtX * (alpha[l,:] * mu[l,:])
    let am: Vec<f64> = (0..data.p)
        .map(|j| model.alpha[l][j] * model.mu[l][j])
        .collect();
    let r_am = xtx_vec(&data.xtx, &am);
    model.xtxr = (0..data.p).map(|j| fitted_without_l[j] + r_am[j]).collect();
}

// ─── trim_null_effects ───────────────────────────────────────────────────────

/// Zero out effects with V < prior_tol.
fn trim_null_effects(params: &SusieParams, model: &mut Model) {
    for l in 0..model.l {
        if model.v[l] < params.prior_tol {
            model.v[l] = 0.0;
            for j in 0..model.p {
                model.alpha[l][j] = params.prior_weights[j];
            }
            model.mu[l] = vec![0.0; model.p];
            model.mu2[l] = vec![0.0; model.p];
            model.lbf_variable[l] = vec![0.0; model.p];
            model.lbf[l] = 0.0;
            model.kl[l] = 0.0;
        }
    }
}

// ─── PIP (susie_get_pip) ─────────────────────────────────────────────────────

/// PIP_j = 1 - Π_l (1 - alpha[l,j])  (for effects with V > prior_tol)
fn compute_pip(model: &Model, prior_tol: f64) -> Vec<f64> {
    let p = model.p;
    let mut pip = vec![0.0; p];
    for j in 0..p {
        let mut prod = 1.0;
        for l in 0..model.l {
            if model.v[l] > prior_tol {
                prod *= 1.0 - model.alpha[l][j];
            }
        }
        pip[j] = 1.0 - prod;
    }
    pip
}
