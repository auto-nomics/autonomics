//! Data preparation: converts RSS inputs (z / bhat+shat, R, n) into the
//! sufficient-statistics form consumed by the IBSS fitting loop.
//!
//! Faithfully follows `summary_stats_constructor` →
//! `summary_stats_working_quantities` → `sufficient_stats_constructor` in
//! susieR's R source.

use crate::SusieError;
use faer::Mat;

// ─── public input structs ────────────────────────────────────────────────────

/// How z-scores relate to the model's σ²=1 scale.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[derive(Default)]
pub enum ZMethod {
    /// Wald z = bhat/shat; PVE-adjusted onto σ²=1 scale (default).
    #[default]
    Wald,
    /// Score z already on σ²=1 scale (e.g. LMM GWAS); no adjustment.
    Score,
}


/// Prior-variance optimization strategy (mirrors susieR's `estimate_prior_method`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[derive(Default)]
pub enum PriorMethod {
    /// R `optim(method="Brent")` on log(V).
    #[default]
    Optim,
    /// One EM update: V = Σ α_j · μ²_j (posterior second moment).
    Em,
    /// Fixed V from `scaled_prior_variance`; no optimization, but applies
    /// the null-threshold check.
    Simple,
}


/// Input bundle for [`crate::susie_rss`].
#[derive(Debug, Clone)]
pub struct RssInput {
    /// z-scores (length p). Provide either `z` or (`bhat`, `shat`).
    pub z: Option<Vec<f64>>,
    /// Estimated marginal effects (length p).
    pub bhat: Option<Vec<f64>>,
    /// Standard errors of `bhat` (length p or scalar broadcast).
    pub shat: Option<Vec<f64>>,
    /// p×p correlation (LD) matrix, row-major.
    pub r: Mat<f64>,
    /// Sample size. Recommended; if `None`, uses large-n approximation.
    pub n: Option<f64>,
    /// Sample variance of y (only used with bhat+shat).
    pub var_y: Option<f64>,
    /// Maximum number of effects.
    pub l: usize,
    /// Prior variance / scaled prior variance.
    pub scaled_prior_variance: f64,
    /// Initial residual variance (overrides default var_y).
    pub residual_variance: Option<f64>,
    /// Estimate residual variance each IBSS iteration?
    pub estimate_residual_variance: bool,
    /// Estimate prior variance per effect?
    pub estimate_prior_variance: bool,
    /// Prior-variance optimization method.
    pub estimate_prior_method: PriorMethod,
    /// z-score method (Wald vs score).
    pub z_method: ZMethod,
    /// Prior inclusion probabilities (length p). Normalized internally.
    pub prior_weights: Option<Vec<f64>>,
    /// Prior probability of no effect (appends a null variable).
    pub null_weight: f64,
    /// Standardize XtX? (Default true — correlation matrix already standardized.)
    pub standardize: bool,
    /// Coverage for credible sets (e.g. 0.95).
    pub coverage: f64,
    /// Min |corr| purity threshold.
    pub min_abs_corr: Option<f64>,
    /// Median |corr| purity threshold (OR-linked with min_abs_corr).
    pub median_abs_corr: Option<f64>,
    /// Prior-variance tolerance: effects with V < this are zeroed.
    pub prior_tol: f64,
    /// Null-threshold: if loglik(0) + threshold ≥ loglik(V̂), set V=0.
    pub check_null_threshold: f64,
    /// Max IBSS iterations.
    pub max_iter: usize,
    /// Convergence tolerance on ELBO.
    pub tol: f64,
}

impl Default for RssInput {
    fn default() -> Self {
        Self {
            z: None,
            bhat: None,
            shat: None,
            r: Mat::zeros(0, 0),
            n: None,
            var_y: None,
            l: 10,
            scaled_prior_variance: 0.2,
            residual_variance: None,
            estimate_residual_variance: false,
            estimate_prior_variance: true,
            estimate_prior_method: PriorMethod::Optim,
            z_method: ZMethod::Wald,
            prior_weights: None,
            null_weight: 0.0,
            standardize: true,
            coverage: 0.95,
            min_abs_corr: Some(0.5),
            median_abs_corr: None,
            prior_tol: 1e-9,
            check_null_threshold: 0.0,
            max_iter: 50,
            tol: 1e-4,
        }
    }
}

// ─── validated data + params ─────────────────────────────────────────────────

/// Sufficient-statistics data object (the `ss` data class in susieR).
#[derive(Debug, Clone)]
pub struct RssData {
    /// p×p standardized XtX (= R for z-only path; (n-1)·R for n path).
    pub xtx: Mat<f64>,
    /// X'y (length p).
    pub xty: Vec<f64>,
    /// y'y.
    pub yty: f64,
    /// Sample size.
    pub n: f64,
    /// Number of variables.
    pub p: usize,
    /// diag(XtX) — the per-variable "d" attribute in susieR.
    pub predictor_weights: Vec<f64>,
    /// Column scale factors (attr "scaled:scale"); all 1 for correlation-R input.
    pub scale_factors: Vec<f64>,
}

/// Build the RSS data object + initial params from user input.
///
/// Mirrors `summary_stats_constructor` → `sufficient_stats_constructor`.
pub fn build_rss_data(
    input: &RssInput,
) -> Result<(RssData, crate::model::SusieParams), SusieError> {
    // ── resolve z from bhat/shat if needed ──
    let (z, bhat_opt, shat_opt) = resolve_z(input)?;

    let p = z.len();
    if input.r.nrows() != p || input.r.ncols() != p {
        return Err(SusieError::DimMismatch {
            z_len: p,
            r_n: input.r.nrows(),
        });
    }

    // ── PVE adjustment ──
    let n_opt = input.n.filter(|&n| n > 1.0);
    let z_method = if bhat_opt.is_some() {
        ZMethod::Wald // bhat/shat are inherently Wald
    } else {
        input.z_method
    };

    let (z_adj, pve_adj) = apply_pve_adjustment(&z, n_opt, z_method);

    // ── compute working quantities (summary_stats_working_quantities) ──
    let prior_variance_default = 50.0; // susieR default for z-only RSS path
    let (xty, yty, n_work, scaled_pv, xtxdiag_opt) = working_quantities(
        &z_adj,
        n_opt,
        bhat_opt.as_deref(),
        shat_opt.as_deref(),
        input.var_y,
        &pve_adj,
        input.scaled_prior_variance,
        prior_variance_default,
    );
    let _ = n_work;

    // ── build XtX ──
    // Three paths:
    //   z-only:       XtX = R, n = 2
    //   standardized: XtX = (n-1)*R
    //   original:     XtX_jk = R_jk * sqrt(XtXdiag_j) * sqrt(XtXdiag_k)
    let (xtx_raw, n_final): (Mat<f64>, f64) = if n_opt.is_none() {
        (input.r.clone(), 2.0)
    } else if let Some(xtxdiag) = &xtxdiag_opt {
        // original_scale path
        let n = n_opt.unwrap();
        let sqrt_d: Vec<f64> = xtxdiag.iter().map(|d| d.sqrt()).collect();
        let xtx = Mat::from_fn(p, p, |i, j| input.r[(i, j)] * sqrt_d[i] * sqrt_d[j]);
        // symmetrize: (XtX + XtX')/2
        let xtx_sym = Mat::from_fn(p, p, |i, j| (xtx[(i, j)] + xtx[(j, i)]) / 2.0);
        (xtx_sym, n)
    } else {
        let nm1 = n_opt.unwrap() - 1.0;
        (&input.r * nm1, n_opt.unwrap())
    };

    // ── standardize XtX (sufficient_stats_constructor) ──
    let (xtx_final, xty_final, scale_factors) = if input.standardize {
        let csd: Vec<f64> = (0..p)
            .map(|j| {
                let s = (xtx_raw[(j, j)] / (n_final - 1.0)).sqrt();
                if s == 0.0 { 1.0 } else { s }
            })
            .collect();
        let xtx_s = Mat::from_fn(p, p, |i, j| xtx_raw[(i, j)] / (csd[i] * csd[j]));
        let xty_s: Vec<f64> = (0..p).map(|j| xty[j] / csd[j]).collect();
        (xtx_s, xty_s, csd)
    } else {
        (xtx_raw, xty, (0..p).map(|_| 1.0).collect::<Vec<_>>())
    };

    // predictor_weights = diag(XtX) = attr(XtX, "d")
    let predictor_weights: Vec<f64> = (0..p).map(|j| xtx_final[(j, j)]).collect();

    // ── null weight / prior weights handling ──
    let (prior_weights, null_weight, add_null) =
        normalize_weights(input.prior_weights.as_deref(), input.null_weight, p);

    // ── assemble final data (possibly with null column appended) ──
    let data = if add_null {
        let mut xtx_null = Mat::zeros(p + 1, p + 1);
        for i in 0..p {
            for j in 0..p {
                xtx_null[(i, j)] = xtx_final[(i, j)];
            }
        }
        let mut xty_null = vec![0.0; p + 1];
        let mut pw_null = vec![0.0; p + 1];
        let mut sf_null = vec![0.0; p + 1];
        for j in 0..p {
            xty_null[j] = xty_final[j];
            pw_null[j] = predictor_weights[j];
            sf_null[j] = scale_factors[j];
        }
        RssData {
            xtx: xtx_null,
            xty: xty_null,
            yty,
            n: n_final,
            p: p + 1,
            predictor_weights: pw_null,
            scale_factors: sf_null,
        }
    } else {
        RssData {
            xtx: xtx_final,
            xty: xty_final,
            yty,
            n: n_final,
            p,
            predictor_weights,
            scale_factors,
        }
    };

    // ── var_y and residual variance ──
    let var_y = data.yty / (data.n - 1.0);
    let residual_variance = input.residual_variance.unwrap_or(var_y);

    let l = input.l.min(data.p).max(1);

    let params = crate::model::SusieParams {
        l,
        scaled_prior_variance: scaled_pv,
        residual_variance,
        prior_weights,
        null_weight,
        estimate_residual_variance: input.estimate_residual_variance,
        estimate_prior_variance: input.estimate_prior_variance,
        estimate_prior_method: input.estimate_prior_method,
        prior_tol: input.prior_tol,
        check_null_threshold: input.check_null_threshold,
        residual_variance_lowerbound: 0.0,
        residual_variance_upperbound: f64::INFINITY,
        coverage: input.coverage,
        min_abs_corr: input.min_abs_corr,
        median_abs_corr: input.median_abs_corr,
        max_iter: input.max_iter,
        tol: input.tol,
        n: data.n,
        p: data.p,
        add_null,
    };

    Ok((data, params))
}

// ─── helpers ─────────────────────────────────────────────────────────────────

/// Resolve z from input: either direct z, or bhat/shat → z = bhat/shat.
fn resolve_z(
    input: &RssInput,
) -> Result<(Vec<f64>, Option<Vec<f64>>, Option<Vec<f64>>), SusieError> {
    let has_z = input.z.is_some();
    let has_bhat = input.bhat.is_some();
    let has_shat = input.shat.is_some();

    if has_z && (has_bhat || has_shat) {
        return Err(SusieError::AmbiguousInput);
    }
    if !has_z && !(has_bhat && has_shat) {
        return Err(SusieError::MissingInput);
    }

    if let Some(z) = &input.z {
        let z_clean: Vec<f64> = z
            .iter()
            .map(|&v| if v.is_nan() { 0.0 } else { v })
            .collect();
        return Ok((z_clean, None, None));
    }

    let bhat = input.bhat.as_ref().unwrap();
    let shat = input.shat.as_ref().unwrap();
    if shat.is_empty() || bhat.is_empty() {
        return Err(SusieError::MissingInput);
    }
    if shat.iter().any(|&s| s <= 0.0) {
        return Err(SusieError::InvalidShat);
    }
    // broadcast scalar shat
    let shat_vec: Vec<f64> = if shat.len() == 1 {
        vec![shat[0]; bhat.len()]
    } else {
        if shat.len() != bhat.len() {
            return Err(SusieError::MissingInput);
        }
        shat.clone()
    };
    let z: Vec<f64> = bhat
        .iter()
        .zip(shat_vec.iter())
        .map(|(&b, &s)| b / s)
        .collect();
    Ok((z, Some(bhat.clone()), Some(shat_vec)))
}

/// PVE adjustment: adj_j = (n-1)/(z_j² + n-2), z_adj = sqrt(adj) * z.
/// For z_method="score", adj=1 (no adjustment).
fn apply_pve_adjustment(z: &[f64], n: Option<f64>, z_method: ZMethod) -> (Vec<f64>, Vec<f64>) {
    let p = z.len();
    match n {
        None => (z.to_vec(), vec![1.0; p]),
        Some(n) if n <= 1.0 => (z.to_vec(), vec![1.0; p]),
        Some(n) => {
            if z_method == ZMethod::Score {
                return (z.to_vec(), vec![1.0; p]);
            }
            let nm1 = n - 1.0;
            let nm2 = n - 2.0;
            let adj: Vec<f64> = z.iter().map(|&zj| nm1 / (zj * zj + nm2)).collect();
            let z_adj: Vec<f64> = z
                .iter()
                .zip(adj.iter())
                .map(|(&zj, &a)| a.sqrt() * zj)
                .collect();
            (z_adj, adj)
        }
    }
}

/// Port of `summary_stats_working_quantities`.
///
/// Returns (Xty, yty, n, scaled_prior_variance, Option<XtXdiag>).
/// XtXdiag is only Some for the original_scale path (bhat+shat+var_y).
#[allow(clippy::too_many_arguments)]
fn working_quantities(
    z: &[f64],
    n: Option<f64>,
    bhat: Option<&[f64]>,
    shat: Option<&[f64]>,
    var_y: Option<f64>,
    pve_adj: &[f64],
    scaled_prior_variance: f64,
    prior_variance_zonly: f64,
) -> (Vec<f64>, f64, f64, f64, Option<Vec<f64>>) {
    let p = z.len();

    // z-only path (no n)
    if n.is_none() {
        return (z.to_vec(), 1.0, 2.0, prior_variance_zonly, None);
    }

    let n = n.unwrap();
    let nm1 = n - 1.0;
    let yty = nm1 * var_y.unwrap_or(1.0);

    // original_scale path: bhat + shat + var_y
    if let (Some(_), Some(s)) = (bhat, shat) {
        let adj = pve_adj;
        let vy = var_y.unwrap_or(1.0);
        // XtXdiag_j = var_y * adj_j / shat_j²
        let xtxdiag: Vec<f64> = (0..p).map(|j| vy * adj[j] / (s[j] * s[j])).collect();
        // Xty_j = z_j * sqrt(adj_j) * var_y / shat_j
        let xty: Vec<f64> = (0..p).map(|j| z[j] * adj[j].sqrt() * vy / s[j]).collect();
        return (xty, yty, n, scaled_prior_variance, Some(xtxdiag));
    }

    // standardized_scale path: z + n (no bhat/shat/var_y)
    let xty: Vec<f64> = z.iter().map(|&zj| nm1.sqrt() * zj).collect();
    (xty, yty, n, scaled_prior_variance, None)
}

/// Normalize prior_weights and null_weight.
///
/// Mirrors susieR's `normalize_null_weight` + `normalize_prior_weights`.
fn normalize_weights(
    prior_weights: Option<&[f64]>,
    null_weight: f64,
    p: usize,
) -> (Vec<f64>, f64, bool) {
    let nw = null_weight.clamp(0.0, 1.0);
    let add_null = nw > 0.0 && nw < 1.0;

    let mut pw: Vec<f64> = match prior_weights {
        Some(pw) if !pw.is_empty() => {
            let s: f64 = pw.iter().sum();
            if s > 0.0 {
                pw.iter().map(|&w| w / s).collect()
            } else {
                vec![1.0 / p as f64; p]
            }
        }
        _ => vec![1.0 / p as f64; p],
    };

    if add_null {
        let s: f64 = pw.iter().sum();
        if s > 0.0 {
            let scale = (1.0 - nw) / s;
            pw = pw.iter().map(|&w| w * scale).collect();
        }
        pw.push(nw);
    }

    // final normalization of the full vector (including null)
    let total: f64 = pw.iter().sum();
    if total > 0.0 {
        pw = pw.iter().map(|&w| w / total).collect();
    }

    (pw, nw, add_null)
}
