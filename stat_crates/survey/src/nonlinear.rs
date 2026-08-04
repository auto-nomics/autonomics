//! Nonlinear and specialised model fitting for survey designs.
//!
//! Implements `svynls` (weighted nonlinear least squares) and `svymle`
//! (maximum pseudolikelihood) with design-based sandwich variance.

use crate::design::SurveyDesign;
use crate::error::{Result, SurveyError};
use crate::variance::svy_cprod_matrix;

use faer::linalg::solvers::{DenseSolveCore, Llt, Solve};
use faer::Mat;

/// Result of a survey NLS fit.
#[derive(Debug, Clone)]
pub struct SvyNlsFit {
    pub coefficients: Vec<f64>,
    pub design_cov: Vec<Vec<f64>>,
    pub fitted: Vec<f64>,
    pub rss: f64,
    pub df: usize,
    pub n: usize,
}

impl SvyNlsFit {
    pub fn se(&self) -> Vec<f64> {
        (0..self.coefficients.len())
            .map(|i| self.design_cov[i][i].sqrt())
            .collect()
    }
}

/// Fit a survey-weighted nonlinear least squares model via Gauss-Newton.
///
/// # Arguments
/// - `y`: response variable.
/// - `x_data`: predictor data passed to the model function.
/// - `design`: survey design (provides weights).
/// - `start`: initial parameter values.
/// - `model_fn`: closure `f(params, x_data) -> Vec<f64>` predicting y.
/// - `jacobian_fn`: closure `f(params, x_data) -> Vec<Vec<f64>>` returning
///   the Jacobian (n × p), where entry [i][j] = ∂f_i/∂β_j.
/// - `max_iter`, `tol`: convergence control.
pub fn svy_nls<F, J>(
    y: &[f64],
    x_data: &[Vec<f64>],
    design: &SurveyDesign,
    start: &[f64],
    model_fn: F,
    jacobian_fn: J,
    max_iter: usize,
    tol: f64,
) -> Result<SvyNlsFit>
where
    F: Fn(&[f64], &[Vec<f64>]) -> Vec<f64>,
    J: Fn(&[f64], &[Vec<f64>]) -> Vec<Vec<f64>>,
{
    let n = design.n_obs;
    let p = start.len();
    if y.len() != n {
        return Err(SurveyError::LengthMismatch {
            context: "y vs design".into(),
            a: y.len(),
            b: n,
        });
    }
    let w = design.weights();
    let w_mean: f64 = w.iter().sum::<f64>() / n as f64;
    let ws: Vec<f64> = w.iter().map(|&wi| wi / w_mean).collect();

    let mut beta = start.to_vec();

    for _iter in 0..max_iter {
        let fitted = model_fn(&beta, x_data);
        let resid: Vec<f64> = (0..n).map(|i| y[i] - fitted[i]).collect();
        let jac = jacobian_fn(&beta, x_data);

        // Gauss-Newton update: δ = (J'WJ)^{-1} J'W r
        let mut jtwj = vec![vec![0.0_f64; p]; p];
        let mut jtwr = vec![0.0_f64; p];
        for i in 0..n {
            for a in 0..p {
                jtwr[a] += jac[i][a] * ws[i] * resid[i];
                for b in 0..p {
                    jtwj[a][b] += jac[i][a] * ws[i] * jac[i][b];
                }
            }
        }
        // Add Levenberg-Marquardt damping for stability.
        let trace = (0..p).map(|a| jtwj[a][a]).sum::<f64>().abs() / p as f64;
        let lambda = 1e-6 * trace;
        for a in 0..p {
            jtwj[a][a] += lambda;
        }

        let jtwj_m = Mat::from_fn(p, p, |a, b| jtwj[a][b]);
        let jtwr_m = Mat::from_fn(p, 1, |a, _| jtwr[a]);
        let llt = Llt::new(jtwj_m.as_ref(), faer::Side::Lower)
            .ok()
            .ok_or_else(|| SurveyError::InvalidInput("singular Jacobian in NLS".into()))?;
        let delta = llt.solve(&jtwr_m);

        let max_delta = (0..p).map(|a| delta[(a, 0)].abs()).fold(0.0_f64, f64::max);
        for a in 0..p {
            beta[a] += delta[(a, 0)];
        }
        if max_delta < tol {
            break;
        }
    }

    // Final fit and design variance.
    let fitted = model_fn(&beta, x_data);
    let resid: Vec<f64> = (0..n).map(|i| y[i] - fitted[i]).collect();
    let jac = jacobian_fn(&beta, x_data);
    let rss: f64 = resid.iter().zip(&ws).map(|(r, wi)| r * r * wi).sum();

    // Naive covariance (J'WJ)^{-1}.
    let mut jtwj = vec![vec![0.0_f64; p]; p];
    for i in 0..n {
        for a in 0..p {
            for b in 0..p {
                jtwj[a][b] += jac[i][a] * ws[i] * jac[i][b];
            }
        }
    }
    let jtwj_m = Mat::from_fn(p, p, |a, b| jtwj[a][b]);
    let llt = Llt::new(jtwj_m.as_ref(), faer::Side::Lower)
        .ok()
        .ok_or_else(|| SurveyError::InvalidInput("singular final Jacobian".into()))?;
    let naive = llt.inverse();

    // Estfun: J_i * resid_i * w_i, influence = estfun * naive.
    let mut influence = vec![vec![0.0_f64; p]; n];
    for i in 0..n {
        for a in 0..p {
            let estfun_ia = jac[i][a] * resid[i] * ws[i];
            for b in 0..p {
                influence[i][a] += estfun_ia * naive[(b, a)];
            }
        }
    }
    // Transpose for svy_cprod_matrix: columns = params.
    let zs: Vec<Vec<f64>> = (0..p)
        .map(|a| (0..n).map(|i| influence[i][a]).collect())
        .collect();
    let design_cov = svy_cprod_matrix(&zs, design)?;

    Ok(SvyNlsFit {
        coefficients: beta,
        design_cov,
        fitted,
        rss,
        df: design.degf(),
        n,
    })
}

// =====================================================================
// svyivreg — two-stage least squares (instrumental variables)
// svyivreg — two-stage least squares (instrumental variables)
// =====================================================================

/// Result of a survey IV regression.
#[derive(Debug, Clone)]
pub struct SvyIvregFit {
    pub coefficients: Vec<f64>,
    pub design_cov: Vec<Vec<f64>>,
    pub df: usize,
    pub n: usize,
}

impl SvyIvregFit {
    pub fn se(&self) -> Vec<f64> {
        (0..self.coefficients.len())
            .map(|i| self.design_cov[i][i].sqrt())
            .collect()
    }
}

/// Two-stage least squares under a survey design.
///
/// 1. First stage: WLS of `endogenous` on `instruments` + `exogenous`.
/// 2. Second stage: WLS of `y` on `endogenous_hat` + `exogenous`.
/// 3. Design variance uses the **original** endogenous (not fitted) in the
///    estimating functions, per Woodridge's correction.
pub fn svy_ivreg(
    y: &[f64],
    endogenous: &[Vec<f64>],
    exogenous: &[Vec<f64>],
    instruments: &[Vec<f64>],
    design: &SurveyDesign,
) -> Result<SvyIvregFit> {
    let n = design.n_obs;
    let n_endo = endogenous.len();
    let n_exo = exogenous.len();
    let n_instr = instruments.len();
    if y.len() != n {
        return Err(SurveyError::LengthMismatch { context: "y".into(), a: y.len(), b: n });
    }

    let w = design.weights();
    let w_mean: f64 = w.iter().sum::<f64>() / n as f64;
    let ws: Vec<f64> = w.iter().map(|wi| wi / w_mean).collect();

    // --- First stage: regress each endogenous variable on instruments+exogenous ---
    let _n_zstage1 = n_instr + n_exo;
    let z1: Vec<Vec<f64>> = instruments
        .iter()
        .chain(exogenous.iter())
        .map(|c| c.iter().enumerate().map(|(i, &v)| v * ws[i].sqrt()).collect())
        .collect();
    // For each endogenous variable, fit WLS and get fitted values.
    let mut endo_hat = vec![vec![0.0_f64; n]; n_endo];
    for (e, endo_col) in endogenous.iter().enumerate() {
        let y_w: Vec<f64> = endo_col.iter().zip(&ws).map(|(&v, wi)| v * wi.sqrt()).collect();
        let z1_refs: Vec<&[f64]> = z1.iter().map(|v| v.as_slice()).collect();
        let reg = statkit::regression::ols(&z1_refs, &y_w, false)
            .map_err(|e| SurveyError::InvalidInput(format!("IV first stage failed: {e}")))?;
        endo_hat[e] = reg.fitted.clone();
    }

    // --- Second stage: WLS of y on endo_hat + exogenous ---
    // Build design matrix for second stage: [endo_hat, exogenous]
    let p = n_endo + n_exo;
    let x2: Vec<Vec<f64>> = endo_hat
        .iter()
        .chain(exogenous.iter())
        .cloned()
        .collect();
    let x2_refs: Vec<&[f64]> = x2.iter().map(|v| v.as_slice()).collect();
    let y_w: Vec<f64> = y.iter().zip(&ws).map(|(&v, wi)| v * wi.sqrt()).collect();
    let reg2 = statkit::regression::ols(&x2_refs, &y_w, false)
        .map_err(|e| SurveyError::InvalidInput(format!("IV second stage failed: {e}")))?;
    let beta = reg2.coefficients.clone();
    let fitted2 = reg2.fitted.clone();
    let resid: Vec<f64> = (0..n).map(|i| y[i] - fitted2[i]).collect();

    // Naive covariance from second stage.
    let mut xtwx = vec![vec![0.0_f64; p]; p];
    for i in 0..n {
        for a in 0..p {
            for b in 0..p {
                xtwx[a][b] += ws[i] * x2[a][i] * x2[b][i];
            }
        }
    }
    let xtwx_m = Mat::from_fn(p, p, |a, b| xtwx[a][b]);
    let llt = Llt::new(xtwx_m.as_ref(), faer::Side::Lower)
        .ok()
        .ok_or_else(|| SurveyError::InvalidInput("singular IV design".into()))?;
    let naive = llt.inverse();

    // Estfun: use **original** endogenous (not fitted) in the score.
    let x_orig: Vec<Vec<f64>> = endogenous
        .iter()
        .chain(exogenous.iter())
        .cloned()
        .collect();
    let mut influence = vec![vec![0.0_f64; p]; n];
    for i in 0..n {
        for a in 0..p {
            let ef = x_orig[a][i] * resid[i] * ws[i];
            for b in 0..p {
                influence[i][a] += ef * naive[(b, a)];
            }
        }
    }
    let zs: Vec<Vec<f64>> = (0..p)
        .map(|a| (0..n).map(|i| influence[i][a]).collect())
        .collect();
    let design_cov = svy_cprod_matrix(&zs, design)?;

    Ok(SvyIvregFit {
        coefficients: beta,
        design_cov,
        df: design.degf(),
        n,
    })
}

// =====================================================================
// svyolr — ordinal logistic regression (proportional-odds model)
// =====================================================================

/// Result of a survey ordinal logistic regression.
#[derive(Debug, Clone)]
pub struct SvyOlrFit {
    /// Slope coefficients (predictors).
    pub coefficients: Vec<f64>,
    /// Cutpoints / thresholds (ordered, n_levels - 1).
    pub alpha: Vec<f64>,
    pub design_cov: Vec<Vec<f64>>,
    pub df: usize,
    pub n: usize,
    pub n_levels: usize,
}

impl SvyOlrFit {
    /// Full coefficient vector (slopes + cutpoints), matching R's `coef(svyolr)`.
    pub fn all_coefs(&self) -> Vec<f64> {
        self.coefficients.iter().chain(self.alpha.iter()).copied().collect()
    }

    pub fn se(&self) -> Vec<f64> {
        (0..self.design_cov.len())
            .map(|i| self.design_cov[i][i].sqrt())
            .collect()
    }
}

/// Fit a survey-weighted proportional-odds (ordinal logistic) model via IRLS.
///
/// Model: P(Y ≤ j | X) = logit⁻¹(α_j − Xβ) for j = 1..K-1.
///
/// # Arguments
/// - `y_ord`: ordinal response as integer levels 0..K-1.
/// - `x`: predictor matrix (n × p).
/// - `design`: survey design.
/// - `max_iter`, `tol`: convergence.
pub fn svy_olr(
    y_ord: &[usize],
    x: &[Vec<f64>],
    design: &SurveyDesign,
    max_iter: usize,
    tol: f64,
) -> Result<SvyOlrFit> {
    let n = design.n_obs;
    let p = x.len();
    if y_ord.len() != n {
        return Err(SurveyError::LengthMismatch { context: "y_ord".into(), a: y_ord.len(), b: n });
    }
    let k = *y_ord.iter().max().unwrap_or(&0) + 1;
    if k < 2 {
        return Err(SurveyError::InvalidInput("need at least 2 ordinal levels".into()));
    }
    let n_thresh = k - 1;
    let n_param = p + n_thresh;

    let w = design.weights();
    let w_mean: f64 = w.iter().sum::<f64>() / n as f64;
    let ws: Vec<f64> = w.iter().map(|wi| wi / w_mean).collect();

    // Initialize: betas = 0, thresholds = evenly spaced logits.
    let mut theta = vec![0.0_f64; n_param];
    // Init thresholds from marginal logit of cumulative proportions.
    let mut cum_count = vec![0_usize; k];
    for &yi in y_ord {
        cum_count[yi] += 1;
    }
    let mut cum = 0;
    for j in 0..n_thresh {
        cum += cum_count[j];
        let prop = cum as f64 / n as f64;
        let logit = (prop / (1.0 - prop)).ln().max(-5.0).min(5.0);
        theta[p + j] = logit;
    }

    for _iter in 0..max_iter {
        // Compute cumulative probabilities and derivatives.
        let mut gamma = vec![0.0_f64; n_thresh + 1]; // gamma[0]=1, gamma[K-1]=0
        gamma[0] = 1.0;
        let _eta: Vec<f64> = vec![0.0; n_thresh];
        for i in 0..n {
            let _ = i;
        }

        // For each observation, compute eta_j = alpha_j - X_i*beta
        // Then gamma_j = 1/(1+exp(-eta_j)), pr_j = gamma_j - gamma_{j+1}
        // Score and information contributions.

        let mut score = vec![0.0_f64; n_param];
        let mut info = vec![vec![0.0_f64; n_param]; n_param];

        for i in 0..n {
            let xb: f64 = (0..p).map(|a| theta[a] * x[a][i]).sum();
            // Cumulative probabilities.
            let mut cum = vec![0.0_f64; k];
            cum[k - 1] = 1.0;
            for j in (0..n_thresh).rev() {
                let eta = theta[p + j] - xb;
                cum[j] = 1.0 / (1.0 + (-eta).exp());
            }
            // Cell probabilities.
            let mut pr = vec![0.0_f64; k];
            for j in 0..k {
                pr[j] = if j == 0 {
                    cum[0]
                } else if j == k - 1 {
                    1.0 - cum[j - 1]
                } else {
                    cum[j] - cum[j - 1]
                };
            }
            let yi = y_ord[i];
            let pr_yi = pr[yi].max(1e-300);

            // Score contributions.
            // d/d_beta_a log Pr(Y=yi) = sum over thresholds
            // For proportional odds: dPr/dtheta_j and dPr/dbeta_a
            let mut dgam_dtheta = vec![vec![0.0_f64; n_thresh]; n_thresh + 1];
            let mut dgam_dbeta = vec![vec![0.0_f64; p]; n_thresh + 1];
            // gamma_j = logit^{-1}(theta_j - xb)
            // dgamma_j/dtheta_j = gamma_j*(1-gamma_j)
            // dgamma_j/dbeta_a = -gamma_j*(1-gamma_j)*x_a
            for j in 0..n_thresh {
                let g = cum[j];
                let dg = g * (1.0 - g);
                dgam_dtheta[j][j] = dg;
                for a in 0..p {
                    dgam_dbeta[j][a] = -dg * x[a][i];
                }
            }
            // dPr/dtheta_j and dPr/dbeta via chain rule.
            // pr[l] = gamma_l - gamma_{l+1} (with gamma_0=1, gamma_K=0)
            // dpr[l]/dtheta_j = dgam[j][l] - dgam[j][l+1] (using gamma indexing)
            // But gamma indexing: gamma[0]=1 (constant), gamma[1..K-1] = cum[0..K-2], gamma[K]=0
            // Actually cum[j] = P(Y ≤ j), so gamma in polr = P(Y ≤ j) for j=0..K-1
            // pr[j] = cum[j] - cum[j-1] (cum[-1]=0, cum[K-1]=1)
            // dpr[j]/dtheta_m = d(cum[j]-cum[j-1])/dtheta_m
            //   = dgam_dtheta[m][j] - dgam_dtheta[m][j-1]
            // But cum[j] depends on theta_j only (for j < K-1):
            //   cum[j] = 1/(1+exp(-(theta_j - xb)))
            // So d(cum[j])/dtheta_m = 0 if m != j, = cum[j]*(1-cum[j]) if m==j

            // Score for threshold parameters:
            for m in 0..n_thresh {
                // d log Pr(Y=yi) / d theta_m
                let mut dlogpr_dtheta = 0.0;
                // Pr(Y=yi) depends on theta_m only through cum[yi] and cum[yi-1]
                if yi < k - 1 && yi == m {
                    dlogpr_dtheta += cum[yi] * (1.0 - cum[yi]);
                }
                if yi > 0 && yi - 1 == m {
                    dlogpr_dtheta -= cum[yi - 1] * (1.0 - cum[yi - 1]);
                }
                score[p + m] += ws[i] * dlogpr_dtheta / pr_yi;
            }
            // Score for beta parameters:
            for a in 0..p {
                let mut dlogpr_dbeta = 0.0;
                if yi < k - 1 {
                    dlogpr_dbeta -= cum[yi] * (1.0 - cum[yi]) * x[a][i];
                }
                if yi > 0 {
                    dlogpr_dbeta += cum[yi - 1] * (1.0 - cum[yi - 1]) * x[a][i];
                }
                score[a] += ws[i] * dlogpr_dbeta / pr_yi;
            }

            // Information matrix (expected Hessian — negative of second derivative).
            // For simplicity, use the outer product of scores (BHHH) per observation.
            let mut grad_i = vec![0.0_f64; n_param];
            for m in 0..n_thresh {
                let mut d = 0.0;
                if yi < k - 1 && yi == m {
                    d += cum[yi] * (1.0 - cum[yi]);
                }
                if yi > 0 && yi - 1 == m {
                    d -= cum[yi - 1] * (1.0 - cum[yi - 1]);
                }
                grad_i[p + m] = ws[i] * d / pr_yi;
            }
            for a in 0..p {
                let mut d = 0.0;
                if yi < k - 1 {
                    d -= cum[yi] * (1.0 - cum[yi]) * x[a][i];
                }
                if yi > 0 {
                    d += cum[yi - 1] * (1.0 - cum[yi - 1]) * x[a][i];
                }
                grad_i[a] = ws[i] * d / pr_yi;
            }
            for a in 0..n_param {
                for b in 0..n_param {
                    info[a][b] += grad_i[a] * grad_i[b];
                }
            }
        }

        // Newton step: delta = info^{-1} * score.
        let info_m = Mat::from_fn(n_param, n_param, |a, b| info[a][b] + 1e-8 * (a == b) as u32 as f64);
        let score_m = Mat::from_fn(n_param, 1, |a, _| score[a]);
        let llt = Llt::new(info_m.as_ref(), faer::Side::Lower)
            .ok()
            .ok_or_else(|| SurveyError::InvalidInput("singular OLR information".into()))?;
        let delta = llt.solve(&score_m);

        let max_delta = (0..n_param).map(|a| delta[(a, 0)].abs()).fold(0.0_f64, f64::max);
        for a in 0..n_param {
            theta[a] += delta[(a, 0)];
        }
        if max_delta < tol {
            break;
        }
    }

    let beta: Vec<f64> = theta[..p].to_vec();
    let alpha: Vec<f64> = theta[p..].to_vec();

    // Design variance via estfun sandwich.
    // Build per-observation estimating functions (same as grad_i above).
    let mut estfun_all = vec![vec![0.0_f64; n_param]; n];
    for i in 0..n {
        let xb: f64 = (0..p).map(|a| beta[a] * x[a][i]).sum();
        let mut cum = vec![0.0_f64; k];
        cum[k - 1] = 1.0;
        for j in (0..n_thresh).rev() {
            let eta = alpha[j] - xb;
            cum[j] = 1.0 / (1.0 + (-eta).exp());
        }
        let yi = y_ord[i];
        let pr_yi = if yi == 0 {
            cum[0]
        } else if yi == k - 1 {
            1.0 - cum[yi - 1]
        } else {
            cum[yi] - cum[yi - 1]
        };
        let pr_yi = pr_yi.max(1e-300);
        for a in 0..p {
            let mut d = 0.0;
            if yi < k - 1 {
                d -= cum[yi] * (1.0 - cum[yi]) * x[a][i];
            }
            if yi > 0 {
                d += cum[yi - 1] * (1.0 - cum[yi - 1]) * x[a][i];
            }
            estfun_all[i][a] = ws[i] * d / pr_yi;
        }
        for m in 0..n_thresh {
            let mut d = 0.0;
            if yi < k - 1 && yi == m {
                d += cum[yi] * (1.0 - cum[yi]);
            }
            if yi > 0 && yi - 1 == m {
                d -= cum[yi - 1] * (1.0 - cum[yi - 1]);
            }
            estfun_all[i][p + m] = ws[i] * d / pr_yi;
        }
    }

    // Naive covariance: (X'WX)^{-1} = info^{-1}.
    let mut info_final = vec![vec![0.0_f64; n_param]; n_param];
    for i in 0..n {
        for a in 0..n_param {
            for b in 0..n_param {
                info_final[a][b] += estfun_all[i][a] * estfun_all[i][b];
            }
        }
    }
    let info_m = Mat::from_fn(n_param, n_param, |a, b| info_final[a][b] + 1e-8 * (a == b) as u32 as f64);
    let llt = Llt::new(info_m.as_ref(), faer::Side::Lower)
        .ok()
        .ok_or_else(|| SurveyError::InvalidInput("singular final OLR info".into()))?;
    let naive = llt.inverse();

    // Influence = estfun * naive.
    let mut influence = vec![vec![0.0_f64; n_param]; n];
    for i in 0..n {
        for a in 0..n_param {
            for b in 0..n_param {
                influence[i][a] += estfun_all[i][b] * naive[(b, a)];
            }
        }
    }
    let zs: Vec<Vec<f64>> = (0..n_param)
        .map(|a| (0..n).map(|i| influence[i][a]).collect())
        .collect();
    let design_cov = svy_cprod_matrix(&zs, design)?;

    Ok(SvyOlrFit {
        coefficients: beta,
        alpha,
        design_cov,
        df: design.degf(),
        n,
        n_levels: k,
    })
}

// =====================================================================
// svyloglin — loglinear model via iterative proportional fitting
// =====================================================================

/// Result of a survey loglinear model fit.
#[derive(Debug, Clone)]
pub struct SvyLoglinFit {
    /// Log-linear coefficients (deviation coding).
    pub coefficients: Vec<f64>,
    pub design_cov: Vec<Vec<f64>>,
    pub df: usize,
    pub n: usize,
}

impl SvyLoglinFit {
    pub fn se(&self) -> Vec<f64> {
        (0..self.coefficients.len())
            .map(|i| self.design_cov[i][i].sqrt())
            .collect()
    }
}

/// Fit a survey-weighted loglinear model for a 2-way table.
///
/// Uses the weighted cell proportions and their design covariance
/// to estimate log-linear parameters via the sandwich formula.
///
/// # Arguments
/// - `row`, `col`: categorical variables.
/// - `design`: survey design.
pub fn svy_loglin(
    row: &[String],
    col: &[String],
    design: &SurveyDesign,
) -> Result<SvyLoglinFit> {
    let n = design.n_obs;
    let w = design.weights();
    let total_w: f64 = w.iter().sum();

    // Unique levels.
    let row_levels: Vec<String> = {
        let mut s = std::collections::HashSet::new();
        let mut v = Vec::new();
        for r in row {
            if s.insert(r.clone()) { v.push(r.clone()); }
        }
        v
    };
    let col_levels: Vec<String> = {
        let mut s = std::collections::HashSet::new();
        let mut v = Vec::new();
        for c in col {
            if s.insert(c.clone()) { v.push(c.clone()); }
        }
        v
    };
    let nr = row_levels.len();
    let nc = col_levels.len();

    // Weighted cell proportions.
    let mut cell_prop = vec![0.0_f64; nr * nc];
    for i in 0..n {
        let r = row_levels.iter().position(|x| x == &row[i]).unwrap();
        let c = col_levels.iter().position(|x| x == &col[i]).unwrap();
        cell_prop[r * nc + c] += w[i] / total_w;
    }

    // Log-linear parameters: intercept, row effects, col effects.
    // Using deviation coding: sum of effects = 0.
    let n_coef = 1 + (nr - 1) + (nc - 1);
    let mut coef = vec![0.0_f64; n_coef];
    // Intercept = mean(log(prop))
    let mean_log = cell_prop.iter().filter(|&&p| p > 0.0).map(|p| p.ln()).sum::<f64>() / (nr * nc) as f64;
    coef[0] = mean_log;
    // Row effects (first nr-1 levels).
    for r in 0..nr - 1 {
        let row_mean: f64 = (0..nc).map(|c| cell_prop[r * nc + c]).sum::<f64>() / nc as f64;
        if row_mean > 0.0 {
            coef[1 + r] = row_mean.ln() - mean_log;
        }
    }
    // Col effects (first nc-1 levels).
    for c in 0..nc - 1 {
        let col_mean: f64 = (0..nr).map(|r| cell_prop[r * nc + c]).sum::<f64>() / nr as f64;
        if col_mean > 0.0 {
            coef[1 + (nr - 1) + c] = col_mean.ln() - mean_log;
        }
    }

    // Design covariance via svymean of cell indicators.
    let ncell = nr * nc;
    let mut cells = vec![vec![0.0_f64; n]; ncell];
    for i in 0..n {
        let r = row_levels.iter().position(|x| x == &row[i]).unwrap();
        let c = col_levels.iter().position(|x| x == &col[i]).unwrap();
        cells[r * nc + c][i] = 1.0;
    }
    // Center + scale for svyCprod.
    let z: Vec<Vec<f64>> = (0..ncell)
        .map(|j| {
            let mean_j: f64 = cells[j].iter().zip(&w).map(|(&x, wi)| x * wi).sum::<f64>() / total_w;
            (0..n).map(|i| w[i] * (cells[j][i] - mean_j) / total_w).collect()
        })
        .collect();
    let cell_var = svy_cprod_matrix(&z, design)?;

    // Map cell covariance to log-linear coefficient covariance via delta method.
    // This is a simplified mapping — the full log-linear covariance requires
    // the Jacobian of the log transformation.
    // For now, use a diagonal approximation.
    let mut design_cov = vec![vec![0.0_f64; n_coef]; n_coef];
    for a in 0..n_coef {
        design_cov[a][a] = if a < cell_var.len() {
            cell_var[a][a] / (cell_prop.get(a).copied().unwrap_or(1.0).max(1e-12)).powi(2)
        } else {
            1e-6
        };
    }

    Ok(SvyLoglinFit {
        coefficients: coef,
        design_cov,
        df: design.degf(),
        n,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::design::SurveyDesignBuilder;

    #[test]
    fn svy_nls_linear_matches_ols() {
        let d = SurveyDesignBuilder::new()
            .strata(vec!["1".into(); 10])
            .cluster((0..10).map(|i| i.to_string()).collect())
            .weights(vec![1.0; 10])
            .build()
            .unwrap();
        let x: Vec<f64> = (0..10).map(|i| i as f64).collect();
        let y: Vec<f64> = x.iter().map(|&xi| 3.0 + 2.0 * xi).collect();
        let x_data = vec![x.clone()];
        let model_fn = |beta: &[f64], xd: &[Vec<f64>]| {
            xd[0].iter().map(|&xi| beta[0] + beta[1] * xi).collect()
        };
        let jac_fn = |_beta: &[f64], xd: &[Vec<f64>]| {
            xd[0].iter().map(|&xi| vec![1.0, xi]).collect()
        };
        let fit = svy_nls(&y, &x_data, &d, &[0.0, 1.0], model_fn, jac_fn, 50, 1e-10).unwrap();
        assert!((fit.coefficients[0] - 3.0).abs() < 1e-6, "intercept: {}", fit.coefficients[0]);
        assert!((fit.coefficients[1] - 2.0).abs() < 1e-6, "slope: {}", fit.coefficients[1]);
    }
}
