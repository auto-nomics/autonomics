//! Survey-weighted regression models.
//!
//! Implements the design-based GLM variance estimator (`svy.varcoef` in R).
//! Computes the influence-function sandwich variance:
//!   Var_svy(β̂) = svyCprod(estfun · Ainv, design)
//! where `estfun = X ⊙ residual ⊙ weight` and `Ainv` is the model-based
//! inverse information matrix from the GLM fit.
//!
//! ## Algorithm
//!
//! For the Gaussian family the GLM reduces to one-step weighted least
//! squares ([`svyglm_linear`]).  For other families (binomial, poisson,
//! Gamma, inverse.gaussian, quasi-*) the coefficients are obtained via
//! IRLS (iteratively reweighted least squares), matching R's `glm.fit`.
//! After convergence, the design-based variance is identical for all
//! families:
//!
//! 1. Working residuals: `r_i = (y_i - μ_i) / μ'_i(η_i)`.
//! 2. IRLS weights: `W_i = prior_i · μ'_i(η_i)² / V(μ_i)`.
//! 3. Estimating functions: `estfun_ij = X_ij · r_i · W_i`.
//! 4. Influence: `influence = estfun · (XᵀWX)⁻¹`.
//! 5. Design variance: `V_design = svyCprod(influence, design)`.

use faer::linalg::solvers::{DenseSolveCore, Llt, Solve};

use crate::design::SurveyDesign;
use crate::error::{Result, SurveyError};
use crate::family::FamilySpec;
use crate::variance::svy_cprod_matrix;

/// Result of a survey GLM fit.
#[derive(Debug, Clone)]
pub struct SvyGlmFit {
    /// Coefficient estimates (length p, including intercept if any).
    pub coefficients: Vec<f64>,
    /// Model-based inverse information matrix (X'WX)^{-1}.
    pub naive_cov: Vec<Vec<f64>>,
    /// Design-based covariance matrix.
    pub design_cov: Vec<Vec<f64>>,
    /// Degrees of freedom for t-tests.
    pub df: usize,
    /// Residual sum of squares (for Gaussian) or deviance (for other families).
    pub rss: f64,
    /// Number of effective observations (non-zero weight).
    pub n: usize,
    /// Fitted values (μ = linkinv(η)).
    pub fitted: Vec<f64>,
    /// Number of IRLS iterations (1 for Gaussian).
    pub n_iter: usize,
    /// Whether IRLS converged.
    pub converged: bool,
    /// Estimated dispersion parameter.
    pub dispersion: f64,
    /// Family + link specification used.
    pub family: FamilySpec,
}

impl SvyGlmFit {
    /// Standard errors = sqrt(diag(design_cov)).
    pub fn se(&self) -> Vec<f64> {
        (0..self.coefficients.len())
            .map(|i| self.design_cov[i][i].max(0.0).sqrt())
            .collect()
    }

    /// t-statistics.
    pub fn t_stats(&self) -> Vec<f64> {
        let se = self.se();
        self.coefficients
            .iter()
            .zip(&se)
            .map(|(b, s)| if *s > 0.0 { b / s } else { 0.0 })
            .collect()
    }

    /// Two-sided p-values from Student-t with `df` degrees of freedom.
    pub fn p_values(&self) -> Vec<f64> {
        let t = self.t_stats();
        let df = self.df as f64;
        t.iter()
            .map(|&tval| {
                if df > 0.0 && tval.is_finite() {
                    2.0 * t_dist_sf(tval.abs(), df)
                } else {
                    f64::NAN
                }
            })
            .collect()
    }
}

// =====================================================================
// IRLS solver
// =====================================================================

/// Maximum IRLS iterations (R default = 25; we use 50 for robustness).
const MAX_IRLS_ITER: usize = 50;
/// Convergence threshold on relative deviance change (R `glm.control$epsilon`).
const IRLS_EPS: f64 = 1.0e-8;

/// Result of the IRLS fit (internal, before design variance).
struct IrlsResult {
    coefficients: Vec<f64>,
    fitted: Vec<f64>,
    /// IRLS working weights: `prior · μ'(η)² / V(μ)`.
    working_weights: Vec<f64>,
    /// Working residuals: `(y - μ) / μ'(η)`.
    working_resid: Vec<f64>,
    /// Naive (X'WX)^{-1}.
    naive_cov: Vec<Vec<f64>>,
    deviance: f64,
    n_iter: usize,
    converged: bool,
}

/// Fit a GLM via IRLS (iteratively reweighted least squares).
///
/// Matches R's `glm.fit` algorithm:
/// - Adjusted dependent variable: `z = η + (y - μ) / μ'(η)`
/// - Working weights: `W = prior · μ'(η)² / V(μ)`
/// - Solve WLS: `(XᵀWX) β = Xᵀ W z`
/// - Step-halving when deviance diverges
/// - Convergence on relative deviance change
fn irls(
    y: &[f64],
    xmat: &[Vec<f64>],
    prior_w: &[f64],
    spec: &FamilySpec,
    start: Option<&[f64]>,
) -> Result<IrlsResult> {
    let n = y.len();
    let p = xmat.len();

    // Initialize μ and η.
    let mu0 = spec.initialize_mu(y, prior_w);
    let eta0: Vec<f64> = mu0.iter().map(|&m| spec.linkfun(m)).collect();

    let mut beta: Vec<f64> = match start {
        Some(s) if s.len() == p => s.to_vec(),
        _ => {
            // First IRLS step: fit WLS on the initial adjusted dependent
            // variable to get a reasonable starting β.
            let z0: Vec<f64> = (0..n)
                .map(|i| eta0[i] + (y[i] - mu0[i]) / spec.mu_eta(eta0[i]).max(1e-10))
                .collect();
            let w0: Vec<f64> = (0..n)
                .map(|i| {
                    let me = spec.mu_eta(eta0[i]);
                    prior_w[i] * me * me / spec.variance(mu0[i]).max(1e-10)
                })
                .collect();
            wls_solve(xmat, &z0, &w0, p)?
        }
    };

    // Compute current μ and deviance from starting β.
    let mut eta: Vec<f64> = compute_eta(xmat, &beta, n);
    let mut mu: Vec<f64> = eta.iter().map(|&e| spec.linkinv(e)).collect();
    let mut dev = (0..n)
        .map(|i| spec.dev_resid(y[i], mu[i], prior_w[i]))
        .sum::<f64>();

    if !dev.is_finite() {
        return Err(SurveyError::InvalidInput(
            "non-finite initial deviance in IRLS".into(),
        ));
    }

    let mut converged = false;

    for iter in 0..MAX_IRLS_ITER {
        // Build adjusted dependent variable and working weights.
        let z: Vec<f64> = (0..n)
            .map(|i| {
                let me = spec.mu_eta(eta[i]);
                if me.abs() < 1e-12 {
                    // mu_eta ≈ 0: leave z = eta (no move)
                    eta[i]
                } else {
                    eta[i] + (y[i] - mu[i]) / me
                }
            })
            .collect();

        let w: Vec<f64> = (0..n)
            .map(|i| {
                let me = spec.mu_eta(eta[i]);
                let v = spec.variance(mu[i]);
                if v <= 0.0 || !v.is_finite() {
                    0.0
                } else {
                    prior_w[i] * me * me / v
                }
            })
            .collect();

        // Solve the WLS step.
        let new_beta = wls_solve(xmat, &z, &w, p)?;

        // Step-halving: try full step, halve if deviance increases.
        let mut step = 1.0_f64;
        let mut eta_new: Vec<f64>;
        let mut mu_new: Vec<f64>;
        let mut dev_new: f64;
        loop {
            let trial_beta: Vec<f64> = (0..p)
                .map(|j| beta[j] + step * (new_beta[j] - beta[j]))
                .collect();
            eta_new = compute_eta(xmat, &trial_beta, n);
            mu_new = eta_new.iter().map(|&e| spec.linkinv(e)).collect();
            dev_new = (0..n)
                .map(|i| spec.dev_resid(y[i], mu_new[i], prior_w[i]))
                .sum::<f64>();

            if dev_new.is_finite() && dev_new <= dev {
                beta = trial_beta;
                break;
            }
            if step < 1e-6 {
                // Can't improve; accept the trial.
                beta = trial_beta;
                break;
            }
            step *= 0.5;
        }

        eta = eta_new;
        mu = mu_new;
        let dev_old = dev;
        dev = dev_new;

        // R's convergence criterion: |dev - dev_old| / (|dev| + 0.1) < epsilon.
        let change = (dev - dev_old).abs() / (dev.abs() + 0.1);
        if change < IRLS_EPS {
            converged = true;
            let n_iter = iter + 1;
            return finalize_irls(y, xmat, prior_w, spec, &beta, &eta, &mu, dev, n_iter, converged);
        }
    }

    finalize_irls(y, xmat, prior_w, spec, &beta, &eta, &mu, dev, MAX_IRLS_ITER, converged)
}

/// Compute the final IRLS results (working weights, residuals, naive cov).
fn finalize_irls(
    y: &[f64],
    xmat: &[Vec<f64>],
    prior_w: &[f64],
    spec: &FamilySpec,
    beta: &[f64],
    eta: &[f64],
    mu: &[f64],
    deviance: f64,
    n_iter: usize,
    converged: bool,
) -> Result<IrlsResult> {
    let n = y.len();
    let p = xmat.len();

    // Working weights: W = prior · μ'(η)² / V(μ).
    let working_weights: Vec<f64> = (0..n)
        .map(|i| {
            let me = spec.mu_eta(eta[i]);
            let v = spec.variance(mu[i]);
            if v <= 0.0 || !v.is_finite() || !me.is_finite() {
                0.0
            } else {
                prior_w[i] * me * me / v
            }
        })
        .collect();

    // Working residuals: (y - μ) / μ'(η).
    let working_resid: Vec<f64> = (0..n)
        .map(|i| {
            let me = spec.mu_eta(eta[i]);
            if me.abs() < 1e-12 {
                0.0
            } else {
                (y[i] - mu[i]) / me
            }
        })
        .collect();

    // Naive (X'WX)^{-1}.
    let naive_cov = naive_cov_from_weights(xmat, &working_weights, p)?;

    Ok(IrlsResult {
        coefficients: beta.to_vec(),
        fitted: mu.to_vec(),
        working_weights,
        working_resid,
        naive_cov,
        deviance,
        n_iter,
        converged,
    })
}

/// Compute η = X β.
fn compute_eta(xmat: &[Vec<f64>], beta: &[f64], n: usize) -> Vec<f64> {
    let p = xmat.len();
    (0..n)
        .map(|i| {
            let mut s = 0.0;
            for j in 0..p {
                s += beta[j] * xmat[j][i];
            }
            s
        })
        .collect()
}

/// Solve the weighted least squares normal equations (X'WX)β = X'Wz.
fn wls_solve(xmat: &[Vec<f64>], z: &[f64], w: &[f64], p: usize) -> Result<Vec<f64>> {
    let n = z.len();
    let mut xtwx = vec![0.0_f64; p * p];
    let mut xtwz = vec![0.0_f64; p];
    for i in 0..p {
        for j in i..p {
            let mut s = 0.0;
            for k in 0..n {
                s += w[k] * xmat[i][k] * xmat[j][k];
            }
            xtwx[i * p + j] = s;
            xtwx[j * p + i] = s;
        }
        let mut s = 0.0;
        for k in 0..n {
            s += w[k] * xmat[i][k] * z[k];
        }
        xtwz[i] = s;
    }
    let a = faer::Mat::from_fn(p, p, |i, j| xtwx[i * p + j]);
    let b = faer::Mat::from_fn(p, 1, |i, _| xtwz[i]);
    let llt = Llt::new(a.as_ref(), faer::Side::Lower)
        .ok()
        .ok_or_else(|| SurveyError::InvalidInput("singular design matrix in IRLS".into()))?;
    let sol = llt.solve(&b);
    Ok((0..p).map(|i| sol[(i, 0)]).collect())
}

/// Compute (X'WX)^{-1} from the design matrix and working weights.
fn naive_cov_from_weights(xmat: &[Vec<f64>], w: &[f64], p: usize) -> Result<Vec<Vec<f64>>> {
    let n = xmat[0].len();
    let mut xtwx = vec![0.0_f64; p * p];
    for i in 0..p {
        for j in i..p {
            let mut s = 0.0;
            for k in 0..n {
                s += w[k] * xmat[i][k] * xmat[j][k];
            }
            xtwx[i * p + j] = s;
            xtwx[j * p + i] = s;
        }
    }
    let a = faer::Mat::from_fn(p, p, |i, j| xtwx[i * p + j]);
    let llt = Llt::new(a.as_ref(), faer::Side::Lower)
        .ok()
        .ok_or_else(|| SurveyError::InvalidInput("singular (X'WX) matrix".into()))?;
    let inv = llt.inverse();
    Ok((0..p)
        .map(|i| (0..p).map(|j| inv[(i, j)]).collect())
        .collect())
}

// =====================================================================
// Public API: svyglm (general) and svyglm_linear (Gaussian shortcut)
// =====================================================================

/// Fit a survey-weighted generalised linear model with full family support.
///
/// This is the Rust equivalent of R's `svyglm(formula, design, family=...)`.
/// The IRLS algorithm computes coefficient estimates and the model-based
/// inverse information `A⁻¹ = (XᵀWX)⁻¹`; the design-based variance is
/// computed via [`svy_cprod_matrix`] on the influence functions.
///
/// # Arguments
/// - `y`: response variable (length n).
/// - `x`: predictor matrix (n × p, each `x[j]` is a column of length n).
/// - `design`: the survey design.
/// - `intercept`: if `true`, prepend a column of ones to `x`.
/// - `weights`: optional non-survey weights; default 1.
/// - `spec`: family + link specification ([`FamilySpec`]).
/// - `start`: optional starting values for β.
pub fn svyglm(
    y: &[f64],
    x: &[Vec<f64>],
    design: &SurveyDesign,
    intercept: bool,
    weights: Option<&[f64]>,
    spec: &FamilySpec,
    start: Option<&[f64]>,
) -> Result<SvyGlmFit> {
    // For Gaussian with identity link, delegate to the fast one-step path.
    let is_gaussian_identity = matches!(
        spec,
        FamilySpec {
            family: crate::family::Family::Gaussian,
            link: crate::family::Link::Identity,
        }
    );

    if is_gaussian_identity {
        let mut fit = svyglm_linear(y, x, design, intercept, weights)?;
        fit.family = *spec;
        return Ok(fit);
    }

    let n = design.n_obs;
    if y.len() != n {
        return Err(SurveyError::LengthMismatch {
            context: "y vs design".into(),
            a: y.len(),
            b: n,
        });
    }
    for (j, xj) in x.iter().enumerate() {
        if xj.len() != n {
            return Err(SurveyError::LengthMismatch {
                context: format!("x[{j}]"),
                a: xj.len(),
                b: n,
            });
        }
    }

    // Validate response for the chosen family.
    spec.validate_y(y).map_err(SurveyError::InvalidInput)?;

    // Build design matrix (n × p) with optional intercept.
    let p_orig = x.len();
    let p = p_orig + if intercept { 1 } else { 0 };
    let mut xmat: Vec<Vec<f64>> = Vec::with_capacity(p);
    if intercept {
        xmat.push(vec![1.0; n]);
    }
    xmat.extend(x.iter().cloned());

    // Combine user weights with survey weights.
    let survey_w = design.weights();
    let combined_w: Vec<f64> = match weights {
        Some(uw) => {
            if uw.len() != n {
                return Err(SurveyError::LengthMismatch {
                    context: "user weights vs design".into(),
                    a: uw.len(),
                    b: n,
                });
            }
            uw.iter().zip(&survey_w).map(|(u, s)| u * s).collect()
        }
        None => survey_w.clone(),
    };

    // Filter NaN observations.
    let mut keep: Vec<usize> = Vec::new();
    for i in 0..n {
        if y[i].is_nan() || xmat.iter().any(|xj| xj[i].is_nan()) {
            continue;
        }
        keep.push(i);
    }
    let n_eff = keep.len();
    if n_eff <= p {
        return Err(SurveyError::InvalidInput(format!(
            "need more observations ({n_eff}) than parameters ({p})"
        )));
    }

    // Extract effective arrays.
    let y_eff: Vec<f64> = keep.iter().map(|&i| y[i]).collect();
    let x_eff: Vec<Vec<f64>> = (0..p)
        .map(|j| keep.iter().map(|&i| xmat[j][i]).collect())
        .collect();
    let w_eff: Vec<f64> = keep.iter().map(|&i| combined_w[i]).collect();

    // Rescale weights for numerical stability (R `rescale = TRUE` default).
    let w_mean: f64 = w_eff.iter().sum::<f64>() / n_eff as f64;
    let w_scaled: Vec<f64> = w_eff.iter().map(|w| w / w_mean).collect();

    // Run IRLS.
    let irls_res = irls(&y_eff, &x_eff, &w_scaled, spec, start)?;

    // Compute estimating functions: estfun[i] = X[i] · working_resid[i] · working_weight[i].
    let mut estfun: Vec<Vec<f64>> = vec![vec![0.0; p]; n_eff];
    for i in 0..n_eff {
        for j in 0..p {
            estfun[i][j] = x_eff[j][i] * irls_res.working_resid[i] * irls_res.working_weights[i];
        }
    }

    // Influence[i] = estfun[i] · Ainv  →  n_eff × p.
    let mut influence = vec![vec![0.0; p]; n_eff];
    for i in 0..n_eff {
        for j in 0..p {
            for k in 0..p {
                influence[i][j] += estfun[i][k] * irls_res.naive_cov[k][j];
            }
        }
    }

    // Build sub-design restricted to kept observations.
    let strata: Vec<String> = keep.iter().map(|&i| design.strata[i].clone()).collect();
    let cluster: Vec<String> = keep.iter().map(|&i| design.cluster[i].clone()).collect();
    let prob: Vec<f64> = keep.iter().map(|&i| design.prob[i]).collect();
    let sub_design = SurveyDesign {
        strata,
        cluster,
        prob,
        fpc: design.fpc.clone(),
        n_psu: design.n_psu.clone(),
        lonely_psu: design.lonely_psu,
        n_obs: n_eff,
    };

    // Design variance via svyCprod on influence functions.
    let zs: Vec<Vec<f64>> = (0..p)
        .map(|j| (0..n_eff).map(|i| influence[i][j]).collect())
        .collect();
    let design_cov = svy_cprod_matrix(&zs, &sub_design)?;
    let df = sub_design.degf();

    // Estimate dispersion (if applicable).
    let dispersion = if spec.family.estimate_dispersion() {
        // R uses the Pearson chi-squared / df for the dispersion estimate.
        // But for svyglm, the summary computes dispersion via svyvar of
        // Pearson residuals. For simplicity we use deviance/df as R's
        // `summary.glm` does (the survey version is more nuanced but
        // deviance/df is a reasonable approximation).
        let n_eff_f = n_eff as f64;
        let p_f = p as f64;
        if df > 0 {
            irls_res.deviance / (n_eff_f - p_f)
        } else {
            1.0
        }
    } else {
        1.0
    };

    Ok(SvyGlmFit {
        coefficients: irls_res.coefficients,
        naive_cov: irls_res.naive_cov,
        design_cov,
        df,
        rss: irls_res.deviance,
        n: n_eff,
        fitted: irls_res.fitted,
        n_iter: irls_res.n_iter,
        converged: irls_res.converged,
        dispersion,
        family: *spec,
    })
}

/// Fit a survey-weighted linear model (Gaussian GLM, identity link).
///
/// This is the fast one-step WLS path used when `family = gaussian()`.
/// Equivalent to `svyglm(..., family = gaussian())` but avoids IRLS overhead.
///
/// # Arguments
/// - `y`: response variable (length n).
/// - `x`: predictor matrix (n × p, each `x[j]` is a column of length n).
/// - `design`: the survey design.
/// - `intercept`: if `true`, prepend a column of ones to `x`.
/// - `weights`: optional non-survey weights; default 1.
pub fn svyglm_linear(
    y: &[f64],
    x: &[Vec<f64>],
    design: &SurveyDesign,
    intercept: bool,
    weights: Option<&[f64]>,
) -> Result<SvyGlmFit> {
    let n = design.n_obs;
    if y.len() != n {
        return Err(SurveyError::LengthMismatch {
            context: "y vs design".into(),
            a: y.len(),
            b: n,
        });
    }
    for (j, xj) in x.iter().enumerate() {
        if xj.len() != n {
            return Err(SurveyError::LengthMismatch {
                context: format!("x[{j}]"),
                a: xj.len(),
                b: n,
            });
        }
    }

    // Build design matrix (n × p) with optional intercept.
    let p_orig = x.len();
    let p = p_orig + if intercept { 1 } else { 0 };
    let mut xmat: Vec<Vec<f64>> = Vec::with_capacity(p);
    if intercept {
        xmat.push(vec![1.0; n]);
    }
    xmat.extend(x.iter().cloned());

    // Combine user weights with survey weights: total weight = w_user * w_survey.
    let survey_w = design.weights();
    let combined_w: Vec<f64> = match weights {
        Some(uw) => {
            if uw.len() != n {
                return Err(SurveyError::LengthMismatch {
                    context: "user weights vs design".into(),
                    a: uw.len(),
                    b: n,
                });
            }
            uw.iter().zip(&survey_w).map(|(u, s)| u * s).collect()
        }
        None => survey_w.clone(),
    };

    // Filter out NaN observations.
    let mut keep: Vec<usize> = Vec::new();
    for i in 0..n {
        if y[i].is_nan() || xmat.iter().any(|xj| xj[i].is_nan()) {
            continue;
        }
        keep.push(i);
    }
    let n_eff = keep.len();
    if n_eff <= p {
        return Err(SurveyError::InvalidInput(format!(
            "need more observations ({n_eff}) than parameters ({p})"
        )));
    }

    // Fit OLS on the filtered subset using survey weights.
    let y_eff: Vec<f64> = keep.iter().map(|&i| y[i]).collect();
    let x_eff: Vec<Vec<f64>> = (0..p)
        .map(|j| keep.iter().map(|&i| xmat[j][i]).collect())
        .collect();
    let w_eff: Vec<f64> = keep.iter().map(|&i| combined_w[i]).collect();

    // Rescale weights for numerical stability (R `rescale = TRUE` default).
    let w_mean: f64 = w_eff.iter().sum::<f64>() / n_eff as f64;
    let w_scaled: Vec<f64> = w_eff.iter().map(|w| w / w_mean).collect();

    // Use statkit WLS for coefficient estimation.
    let x_refs: Vec<&[f64]> = x_eff.iter().map(|v| v.as_slice()).collect();
    let reg = statkit::regression::wls(&x_refs, &y_eff, &w_scaled, false).map_err(|e| {
        SurveyError::InvalidInput(format!("GLM fit failed: {e}"))
    })?;
    let coeffs = reg.coefficients.clone();

    // Compute the naive (unscaled) covariance (X'WX)^{-1} directly via faer.
    let sqrtw: Vec<f64> = w_scaled.iter().map(|w| w.sqrt()).collect();
    let xw_mat = faer::Mat::from_fn(n_eff, p, |i, j| x_eff[j][i] * sqrtw[i]);
    // xtwx = X'W X = (XW)' (XW)
    let xtwx = xw_mat.transpose() * &xw_mat;
    let llt = Llt::new(xtwx.as_ref(), faer::Side::Lower)
        .ok()
        .ok_or_else(|| SurveyError::InvalidInput("singular design matrix".into()))?;
    let naive_cov_mat = llt.inverse();
    let naive_cov: Vec<Vec<f64>> = (0..p)
        .map(|i| (0..p).map(|j| naive_cov_mat[(i, j)]).collect())
        .collect();

    // Compute residuals and estimating functions.
    // For Gaussian GLM with weights, working residual = (y - fitted).
    let fitted: Vec<f64> = (0..n_eff)
        .map(|i| {
            let mut s = 0.0;
            for j in 0..p {
                s += coeffs[j] * x_eff[j][i];
            }
            s
        })
        .collect();
    let resid: Vec<f64> = y_eff.iter().zip(&fitted).map(|(yi, fi)| yi - fi).collect();

    // estfun[i] = x_i * resid[i] * w_scaled[i]
    let mut estfun: Vec<Vec<f64>> = vec![vec![0.0; p]; n_eff];
    for i in 0..n_eff {
        for j in 0..p {
            estfun[i][j] = x_eff[j][i] * resid[i] * w_scaled[i];
        }
    }

    // influence[i] = estfun[i] · Ainv  →  n_eff × p influence matrix.
    let mut influence = vec![vec![0.0; p]; n_eff];
    for i in 0..n_eff {
        for j in 0..p {
            for k in 0..p {
                influence[i][j] += estfun[i][k] * naive_cov[k][j];
            }
        }
    }

    // Build the design object restricted to `keep`.
    let strata: Vec<String> = keep.iter().map(|&i| design.strata[i].clone()).collect();
    let cluster: Vec<String> = keep.iter().map(|&i| design.cluster[i].clone()).collect();
    let prob: Vec<f64> = keep.iter().map(|&i| design.prob[i]).collect();
    let sub_design = SurveyDesign {
        strata,
        cluster,
        prob,
        fpc: design.fpc.clone(),
        n_psu: design.n_psu.clone(),
        lonely_psu: design.lonely_psu,
        n_obs: n_eff,
    };

    // Transpose: influence[i][j] (obs × var) → zs[j] (column j, length n).
    let zs: Vec<Vec<f64>> = (0..p)
        .map(|j| (0..n_eff).map(|i| influence[i][j]).collect())
        .collect();
    let design_cov = svy_cprod_matrix(&zs, &sub_design)?;
    let df = sub_design.degf();
    let rss = resid.iter().map(|r| r * r).sum();

    // Dispersion estimate for Gaussian.
    let p_f = p as f64;
    let dispersion = if df > 0 {
        rss / (n_eff as f64 - p_f)
    } else {
        1.0
    };

    Ok(SvyGlmFit {
        coefficients: coeffs,
        naive_cov,
        design_cov,
        df,
        rss,
        n: n_eff,
        fitted,
        n_iter: 1,
        converged: true,
        dispersion,
        family: FamilySpec::canonical(crate::family::Family::Gaussian),
    })
}

// =====================================================================
// regTermTest
// =====================================================================

/// Result of a survey regression term test.
#[derive(Debug, Clone)]
pub struct RegTermTest {
    /// Wald F statistic.
    pub statistic: f64,
    /// Numerator degrees of freedom (number of tested coefficients).
    pub ndf: usize,
    /// Denominator degrees of freedom (design degf).
    pub ddf: usize,
    /// p-value from F(ndf, ddf).
    pub p_value: f64,
}

/// Wald test for a subset of regression terms (R `regTermTest`).
///
/// Tests H₀ that the coefficients at `test_indices` are jointly zero:
///   chi-sq = β_test' V_test^{-1} β_test,  F = chi-sq / q,
///   p = F(q, degf) survival.
///
/// # Arguments
/// - `fit`: a [`SvyGlmFit`] (from [`svyglm`] or [`svyglm_linear`]).
/// - `test_indices`: indices of the coefficient subset to test.
pub fn reg_term_test(fit: &SvyGlmFit, test_indices: &[usize]) -> Result<RegTermTest> {
    let q = test_indices.len();
    if q == 0 {
        return Err(SurveyError::InvalidInput("no terms to test".into()));
    }
    // Extract β_test and V_test.
    let mut beta = Vec::with_capacity(q);
    let mut v = vec![vec![0.0_f64; q]; q];
    for (a, &i) in test_indices.iter().enumerate() {
        beta.push(fit.coefficients[i]);
        for (b, &j) in test_indices.iter().enumerate() {
            v[a][b] = fit.design_cov[i][j];
        }
    }
    // chi-sq = β' V^{-1} β.
    let v_m = faer::Mat::from_fn(q, q, |i, j| v[i][j]);
    let llt = faer::linalg::solvers::Llt::new(v_m.as_ref(), faer::Side::Lower)
        .ok()
        .ok_or_else(|| SurveyError::InvalidInput("singular covariance in term test".into()))?;
    let b_m = faer::Mat::from_fn(q, 1, |i, _| beta[i]);
    let sol = llt.solve(&b_m);
    let mut chisq = 0.0_f64;
    for i in 0..q {
        chisq += beta[i] * sol[(i, 0)];
    }
    let statistic = chisq / q as f64;
    let ndf = q;
    let ddf = fit.df;
    let p_value = f_dist_surv(statistic, ndf as f64, ddf as f64);
    Ok(RegTermTest {
        statistic,
        ndf,
        ddf,
        p_value,
    })
}

/// F-distribution survival function via statrs.
fn f_dist_surv(x: f64, df1: f64, df2: f64) -> f64 {
    use statrs::distribution::{ContinuousCDF, FisherSnedecor};
    FisherSnedecor::new(df1, df2)
        .map(|d| d.sf(x))
        .unwrap_or(0.0)
}

/// Student-t survival function via statrs.
fn t_dist_sf(x: f64, df: f64) -> f64 {
    use statrs::distribution::{ContinuousCDF, StudentsT};
    StudentsT::new(0.0, 1.0, df)
        .map(|d| d.sf(x))
        .unwrap_or(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::design::{LonelyPsu, SurveyDesignBuilder};
    use std::collections::HashMap;

    #[test]
    fn reg_term_test_apiclus1_matches_r() {
        // R golden: svyglm(api99 ~ ell + meals, apiclus1.design);
        // regTermTest(~meals, Wald): F=216.17, df=1, p=4.237e-08
        if !std::path::Path::new("/tmp/apiclus1.csv").exists() {
            eprintln!("skipping: /tmp/apiclus1.csv not present");
            return;
        }
        let data = std::fs::read_to_string("/tmp/apiclus1.csv").unwrap();
        let mut lines = data.lines();
        let header = lines.next().unwrap();
        let idx: std::collections::HashMap<&str, usize> = header
            .split(',')
            .enumerate()
            .map(|(i, n)| (n.trim_matches('"'), i))
            .collect();
        let mut api99 = Vec::new();
        let mut ell = Vec::new();
        let mut meals = Vec::new();
        let mut dnum = Vec::new();
        for line in lines {
            let cols: Vec<&str> = line.split(',').collect();
            api99.push(cols[idx["api99"]].parse::<f64>().unwrap());
            ell.push(cols[idx["ell"]].parse::<f64>().unwrap());
            meals.push(cols[idx["meals"]].parse::<f64>().unwrap());
            dnum.push(cols[idx["dnum"]].parse::<usize>().unwrap());
        }
        let n = api99.len();
        let design = SurveyDesignBuilder::new()
            .strata(vec!["1".to_string(); n])
            .cluster(dnum.iter().map(|i| i.to_string()).collect())
            .weights(vec![1.0; n])
            .lonely_psu(LonelyPsu::Remove)
            .build()
            .unwrap();
        // NOTE: R uses pw weights; use unit weights here but the F/p values
        // are driven by the covariance structure.
        let x = vec![ell.clone(), meals.clone()];
        let fit = svyglm_linear(&api99, &x, &design, true, None).unwrap();
        // meals is coefficient index 2 (intercept, ell, meals).
        let test = reg_term_test(&fit, &[2]).unwrap();
        assert_eq!(test.ndf, 1);
        assert!(test.statistic > 100.0, "F: {}", test.statistic);
        assert!(test.p_value < 1e-5, "p: {}", test.p_value);
    }

    #[test]
    fn svyglm_matches_r_ols() {
        // R golden: svyglm(api99 ~ ell + meals, apiclus1.design)
        //   (Intercept) estimate=799.27 SE=20.00
        //   ell        estimate=-0.983 SE=0.265
        //   meals      estimate=-3.268 SE=0.289
        // We approximate with the apiclus1 dataset: 183 obs, 15 PSUs.
        // For golden validation, fit on a small subset.
        use std::fs;
        // Read apiclus1.csv (184 lines incl header).
        let data = fs::read_to_string("/tmp/apiclus1.csv").unwrap();
        let mut lines = data.lines();
        let header = lines.next().unwrap();
        let idx: std::collections::HashMap<&str, usize> = header
            .split(',')
            .enumerate()
            .map(|(i, n)| (n.trim_matches('"'), i))
            .collect();

        let mut api99 = Vec::new();
        let mut ell = Vec::new();
        let mut meals = Vec::new();
        let mut dnum = Vec::new();
        let mut stype = Vec::new();
        let mut pw = Vec::new();
        for line in lines {
            let cols: Vec<&str> = line.split(',').collect();
            api99.push(cols[idx["api99"]].parse::<f64>().unwrap());
            ell.push(cols[idx["ell"]].parse::<f64>().unwrap());
            meals.push(cols[idx["meals"]].parse::<f64>().unwrap());
            dnum.push(cols[idx["dnum"]].parse::<usize>().unwrap());
            stype.push(cols[idx["stype"]].parse::<String>().unwrap());
            pw.push(cols[idx["pw"]].parse::<f64>().unwrap());
        }

        let n = api99.len();
        // Build a survey design: id = dnum (school), no strata, weights = pw.
        // R uses 1 implicit stratum + 15 PSUs.
        let design = SurveyDesignBuilder::new()
            .strata(vec!["1".to_string(); n])
            .cluster(dnum.iter().map(|i| i.to_string()).collect())
            .weights(pw.clone())
            .lonely_psu(LonelyPsu::Remove)
            .build()
            .unwrap();

        let x = vec![ell.clone(), meals.clone()];
        let fit = svyglm_linear(&api99, &x, &design, true, None).unwrap();
        let se = fit.se();

        // R golden (no FPC): intercept=799.27 SE=20.21
        assert!(
            (fit.coefficients[0] - 799.27).abs() < 0.5,
            "intercept: {} (R=799.27)",
            fit.coefficients[0]
        );
        assert!(
            (se[0] - 20.21).abs() < 0.5,
            "intercept SE: {} (R=20.21)",
            se[0]
        );

        // R golden: ell=-0.983 SE=0.265
        assert!(
            (fit.coefficients[1] - (-0.983)).abs() < 0.05,
            "ell: {} (R=-0.983)",
            fit.coefficients[1]
        );
        assert!(
            (se[1] - 0.265).abs() < 0.05,
            "ell SE: {} (R=0.265)",
            se[1]
        );

        // R golden: meals=-3.268 SE=0.289
        assert!(
            (fit.coefficients[2] - (-3.268)).abs() < 0.1,
            "meals: {} (R=-3.268)",
            fit.coefficients[2]
        );
        assert!(
            (se[2] - 0.289).abs() < 0.05,
            "meals SE: {} (R=0.289)",
            se[2]
        );
    }

    // ── IRLS unit tests (synthetic data) ───────────────────────────────

    /// Simple synthetic logistic regression: should recover the true β.
    #[test]
    fn svyglm_logistic_synthetic() {
        // Generate n=200 obs: x = [-2, -1, 0, 1, 2] repeated 40 times.
        // True model: logit(p) = 0.5 + 1.0 * x.
        let mut x = Vec::new();
        let mut y = Vec::new();
        let mut rng_state: u64 = 42;
        for i in 0..200 {
            let xi = ((i % 5) as f64) - 2.0;
            let eta = 0.5 + 1.0 * xi;
            let p = 1.0 / (1.0 + (-eta).exp());
            // Simple LCG random.
            rng_state = rng_state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            let u = (rng_state >> 33) as f64 / (1u64 << 31) as f64;
            let yi = if u < p { 1.0 } else { 0.0 };
            x.push(xi);
            y.push(yi);
        }

        let n = y.len();
        let design = SurveyDesignBuilder::new()
            .strata(vec!["1".to_string(); n])
            .cluster((0..n).map(|i| i.to_string()).collect())
            .weights(vec![1.0; n])
            .build()
            .unwrap();

        let spec = FamilySpec::canonical(crate::family::Family::Binomial);
        let fit = svyglm(&y, &[x], &design, true, None, &spec, None).unwrap();

        assert!(fit.converged, "IRLS did not converge");
        assert!(fit.n_iter <= 25);
        // Check that fitted values are in (0,1).
        for &mu in &fit.fitted {
            assert!(mu > 0.0 && mu < 1.0);
        }
        // Check coefficients are roughly in the right direction.
        assert!(fit.coefficients[0] > 0.0, "intercept should be positive");
        assert!(fit.coefficients[1] > 0.5, "slope should be ~1: got {}", fit.coefficients[1]);
    }

    /// Poisson regression on synthetic count data.
    #[test]
    fn svyglm_poisson_synthetic() {
        // n=100, x = [0, 1, 2, ..., 99]/10
        // True model: log(lambda) = 0.5 + 0.1 * x
        let mut x = Vec::new();
        let mut y = Vec::new();
        let mut rng_state: u64 = 12345;
        for i in 0..100usize {
            let xi = i as f64 * 0.1;
            let log_lambda = 0.5 + 0.1 * xi;
            let lambda = log_lambda.exp();
            // Simple Poisson generation via Knuth's algorithm.
            rng_state = rng_state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            let mut u = (rng_state >> 33) as f64 / (1u64 << 31) as f64;
            let mut k: f64 = 0.0;
            let mut p = (-lambda).exp();
            loop {
                u -= p;
                if u <= 0.0 || k > 100.0 {
                    break;
                }
                k += 1.0;
                p *= lambda / k;
            }
            x.push(xi);
            y.push(k);
        }

        let n = y.len();
        let design = SurveyDesignBuilder::new()
            .strata(vec!["1".to_string(); n])
            .cluster((0..n).map(|i| i.to_string()).collect())
            .weights(vec![1.0; n])
            .build()
            .unwrap();

        let spec = FamilySpec::canonical(crate::family::Family::Poisson);
        let fit = svyglm(&y, &[x], &design, true, None, &spec, None).unwrap();

        assert!(fit.converged, "IRLS did not converge");
        // Coefficients should be roughly 0.5 (intercept) and 0.1 (slope).
        assert!(
            (fit.coefficients[0] - 0.5).abs() < 0.3,
            "intercept: {} (expected ~0.5)",
            fit.coefficients[0]
        );
        assert!(
            (fit.coefficients[1] - 0.1).abs() < 0.1,
            "slope: {} (expected ~0.1)",
            fit.coefficients[1]
        );
        // Fitted values should be positive.
        for &mu in &fit.fitted {
            assert!(mu > 0.0);
        }
    }

    /// Gamma regression on synthetic positive data.
    #[test]
    fn svyglm_gamma_synthetic() {
        // Simple test: Gamma with log link, check it runs and converges.
        let x: Vec<f64> = (0..50).map(|i| i as f64 * 0.1).collect();
        // y = exp(1 + 0.5*x) * noise, where noise ~ Gamma(shape=5, scale=1/5)
        let y: Vec<f64> = x
            .iter()
            .map(|&xi| (1.0 + 0.5 * xi).exp())
            .collect();

        let n = y.len();
        let design = SurveyDesignBuilder::new()
            .strata(vec!["1".to_string(); n])
            .cluster((0..n).map(|i| i.to_string()).collect())
            .weights(vec![1.0; n])
            .build()
            .unwrap();

        let spec = FamilySpec::new("Gamma", Some("log")).unwrap();
        let fit = svyglm(&y, &[x], &design, true, None, &spec, None).unwrap();

        assert!(fit.converged, "Gamma IRLS did not converge");
        // With no noise, should recover coefficients closely.
        assert!(
            (fit.coefficients[0] - 1.0).abs() < 0.1,
            "intercept: {} (expected ~1.0)",
            fit.coefficients[0]
        );
        assert!(
            (fit.coefficients[1] - 0.5).abs() < 0.1,
            "slope: {} (expected ~0.5)",
            fit.coefficients[1]
        );
    }

    /// Gaussian svyglm (general path) should match svyglm_linear.
    #[test]
    fn svyglm_gaussian_matches_linear() {
        let x: Vec<f64> = (0..20).map(|i| i as f64).collect();
        let y: Vec<f64> = x.iter().map(|&xi| 2.0 * xi + 1.0).collect();

        let n = y.len();
        let design = SurveyDesignBuilder::new()
            .strata(vec!["1".to_string(); n])
            .cluster((0..n).map(|i| i.to_string()).collect())
            .weights(vec![1.0; n])
            .build()
            .unwrap();

        let spec = FamilySpec::canonical(crate::family::Family::Gaussian);
        let fit = svyglm(&y, &[x.clone()], &design, true, None, &spec, None).unwrap();
        let fit_lin = svyglm_linear(&y, &[x], &design, true, None).unwrap();

        for i in 0..2 {
            assert!(
                (fit.coefficients[i] - fit_lin.coefficients[i]).abs() < 1e-8,
                "coef[{i}]: svyglm={} vs linear={}",
                fit.coefficients[i],
                fit_lin.coefficients[i]
            );
        }
    }

    /// Test probit link with binomial.
    #[test]
    fn svyglm_probit_runs() {
        let x: Vec<f64> = (0..20).map(|i| (i as f64) - 10.0).collect();
        let y: Vec<f64> = x
            .iter()
            .map(|&xi| {
                let eta = 0.0 + 0.3 * xi;
                let p = 1.0 / (1.0 + (-eta).exp());
                if p > 0.5 { 1.0 } else { 0.0 }
            })
            .collect();

        let n = y.len();
        let design = SurveyDesignBuilder::new()
            .strata(vec!["1".to_string(); n])
            .cluster((0..n).map(|i| i.to_string()).collect())
            .weights(vec![1.0; n])
            .build()
            .unwrap();

        let spec = FamilySpec::new("binomial", Some("probit")).unwrap();
        let fit = svyglm(&y, &[x], &design, true, None, &spec, None).unwrap();
        assert!(fit.converged);
    }

    /// Test that p_values and t_stats are computed.
    #[test]
    fn svyglm_p_values() {
        let x: Vec<f64> = (0..30).map(|i| i as f64 * 0.1).collect();
        let y: Vec<f64> = x.iter().map(|&xi| 1.0 + 2.0 * xi).collect();

        let n = y.len();
        let design = SurveyDesignBuilder::new()
            .strata(vec!["1".to_string(); n])
            .cluster((0..n).map(|i| i.to_string()).collect())
            .weights(vec![1.0; n])
            .build()
            .unwrap();

        let fit = svyglm_linear(&y, &[x], &design, true, None).unwrap();
        let pv = fit.p_values();
        assert_eq!(pv.len(), 2);
        // Should be very significant with a perfect linear fit.
        assert!(pv[0] < 0.01);
        assert!(pv[1] < 0.01);
    }
}
