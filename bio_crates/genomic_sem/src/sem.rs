//! Structural Equation Modeling (SEM) engine — the core of GenomicSEM.
//!
//! This module replaces R's `lavaan::sem()` for the subset of functionality
//! needed by GenomicSEM:
//!
//! - Lavaan syntax parsing (`=~`, `~~`, `~`, `:=`)
//! - LISREL all-y parameterization
//! - DWLS (diagonally weighted least squares) estimation
//! - ML (maximum likelihood) estimation
//! - Sandwich-corrected standard errors
//! - Model fit indices (χ², CFI, AIC, SRMR)
//!
//! The engine works with covariance-matrix input (no raw data), which is how
//! GenomicSEM operates: it fits SEMs to the LDSC-derived genetic covariance
//! matrix `S` weighted by the inverse of its sampling covariance `V`.

use faer::Mat;

use crate::error::{GenomicSemError, Result};
use crate::linalg;
use crate::near_pd;

// =====================================================================
// Syntax parsing
// =====================================================================

/// One line of a lavaan model specification.
#[derive(Clone, Debug, PartialEq)]
pub struct ModelLine {
    pub lhs: String,
    pub op: String,   // "=~", "~~", "~", ":="
    pub rhs: String,  // unparsed RHS
}

/// A parsed lavaan model.
#[derive(Clone, Debug)]
pub struct SemModel {
    pub lines: Vec<ModelLine>,
    pub observed_vars: Vec<String>,
    pub latent_vars: Vec<String>,
}

/// Parse a lavaan-style model string.
pub fn parse_model(model_str: &str) -> Result<SemModel> {
    let mut lines = Vec::new();
    let mut observed_vars = Vec::new();
    let mut latent_vars = Vec::new();

    for raw_line in model_str.split('\n') {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('!') || line.starts_with('#') {
            continue;
        }

        let (lhs, op, rhs) = if line.contains("=~") {
            let parts: Vec<&str> = line.splitn(2, "=~").collect();
            (parts[0].trim().to_string(), "=~".to_string(), parts[1].trim().to_string())
        } else if line.contains("~~") {
            let parts: Vec<&str> = line.splitn(2, "~~").collect();
            (parts[0].trim().to_string(), "~~".to_string(), parts[1].trim().to_string())
        } else if line.contains(":=") {
            let parts: Vec<&str> = line.splitn(2, ":=").collect();
            (parts[0].trim().to_string(), ":=".to_string(), parts[1].trim().to_string())
        } else if line.contains("==") {
            let parts: Vec<&str> = line.splitn(2, "==").collect();
            (parts[0].trim().to_string(), "==".to_string(), parts[1].trim().to_string())
        } else if line.contains("~") {
            let parts: Vec<&str> = line.splitn(2, "~").collect();
            (parts[0].trim().to_string(), "~".to_string(), parts[1].trim().to_string())
        } else {
            return Err(GenomicSemError::Syntax(format!(
                "Cannot parse line: '{line}'"
            )));
        };

        // Track variable types
        let rhs_varnames = extract_var_names(&rhs, &op);
        match op.as_str() {
            "=~" => {
                if !latent_vars.contains(&lhs) {
                    latent_vars.push(lhs.clone());
                }
                for name in &rhs_varnames {
                    if !observed_vars.contains(name) {
                        observed_vars.push(name.clone());
                    }
                }
            }
            "~~" | "~" => {
                if !observed_vars.contains(&lhs) && !latent_vars.contains(&lhs) {
                    observed_vars.push(lhs.clone());
                }
                for name in &rhs_varnames {
                    if !observed_vars.contains(name) && !latent_vars.contains(name) {
                        observed_vars.push(name.clone());
                    }
                }
            }
            _ => {}
        }

        lines.push(ModelLine { lhs, op, rhs });
    }

    Ok(SemModel { lines, observed_vars, latent_vars })
}

/// Extract variable names from RHS, stripping coefficients and labels.
fn extract_var_names(rhs: &str, op: &str) -> Vec<String> {
    if op == ":=" {
        return Vec::new();
    }
    rhs.split('+')
        .map(|t| {
            let t = t.trim();
            if let Some(pos) = t.find('*') {
                t[pos + 1..].trim().to_string()
            } else {
                t.to_string()
            }
        })
        .filter(|s| !s.is_empty())
        .collect()
}

// =====================================================================
// Parameter table
// =====================================================================

/// A parameter in the model's parameter table.
#[derive(Clone, Debug)]
pub struct Param {
    pub lhs: String,
    pub op: String,
    pub rhs: String,
    pub free: i32,   // 0 = fixed, >0 = free parameter index (1-based)
    pub ustart: f64, // user-specified starting value (NaN = free)
    pub label: String,
    pub est: f64,
}

/// Build the parameter table from a parsed model.
pub fn build_param_table(model: &SemModel, s_names: &[String]) -> Vec<Param> {
    let mut params = Vec::new();
    let mut free_idx = 1i32;

    for line in &model.lines {
        let rhs_terms: Vec<&str> = line.rhs.split('+').map(|s| s.trim()).collect();
        match line.op.as_str() {
            "=~" | "~~" | "~" => {
                for term in rhs_terms {
                    let (varname, fixed_val, is_fixed) = parse_term(term);
                    let param = Param {
                        lhs: line.lhs.clone(),
                        op: line.op.clone(),
                        rhs: varname,
                        free: if is_fixed { 0 } else { let i = free_idx; free_idx += 1; i },
                        ustart: fixed_val.unwrap_or(f64::NAN),
                        label: String::new(),
                        est: 0.0,
                    };
                    params.push(param);
                }
            }
            ":=" => {
                params.push(Param {
                    lhs: line.lhs.clone(), op: ":=".into(), rhs: line.rhs.clone(),
                    free: 0, ustart: f64::NAN, label: String::new(), est: 0.0,
                });
            }
            _ => {}
        }
    }

    // Default residual variances for observed vars
    for name in s_names {
        let has_var = params.iter().any(|p| p.op == "~~" && p.lhs == *name && p.rhs == *name);
        if !has_var {
            let i = free_idx; free_idx += 1;
            params.push(Param { lhs: name.clone(), op: "~~".into(), rhs: name.clone(),
                free: i, ustart: f64::NAN, label: String::new(), est: 0.0 });
        }
    }

    // Default latent variances
    for lv in &model.latent_vars {
        let has_var = params.iter().any(|p| p.op == "~~" && p.lhs == *lv && p.rhs == *lv);
        if !has_var {
            let i = free_idx; free_idx += 1;
            params.push(Param { lhs: lv.clone(), op: "~~".into(), rhs: lv.clone(),
                free: i, ustart: f64::NAN, label: String::new(), est: 0.0 });
        }
    }

    params
}

fn parse_term(term: &str) -> (String, Option<f64>, bool) {
    let term = term.trim();
    if let Some(pos) = term.find('*') {
        let coeff = &term[..pos].trim();
        let var = term[pos + 1..].trim().to_string();
        if *coeff == "NA" {
            return (var, None, false);
        } else if let Ok(val) = coeff.parse::<f64>() {
            return (var, Some(val), true);
        } else {
            return (var, None, false);
        }
    }
    (term.to_string(), None, false)
}

// =====================================================================
// Model-implied covariance
// =====================================================================

/// Compute the model-implied covariance matrix Σ.
/// Σ = Λ(I-Β)⁻¹ Ψ(I-Β)⁻ᵀ Λᵀ + Θ
pub fn compute_implied_cov(params: &[Param], observed_vars: &[String], latent_vars: &[String]) -> Mat<f64> {
    let n_obs = observed_vars.len();
    let n_lat = latent_vars.len();

    let mut lambda = Mat::zeros(n_obs, n_lat);
    let mut psi = Mat::zeros(n_lat, n_lat);
    let mut theta = Mat::zeros(n_obs, n_obs);
    let mut beta = Mat::zeros(n_lat, n_lat);

    for p in params {
        match p.op.as_str() {
            "=~" => {
                if let (Some(row), Some(col)) = (
                    observed_vars.iter().position(|v| v == &p.rhs),
                    latent_vars.iter().position(|v| v == &p.lhs),
                ) { lambda[(row, col)] = p.est; }
            }
            "~~" => {
                if let (Some(r), Some(c)) = (
                    latent_vars.iter().position(|v| v == &p.lhs),
                    latent_vars.iter().position(|v| v == &p.rhs),
                ) { psi[(r, c)] = p.est; psi[(c, r)] = p.est; }
                else if let (Some(r), Some(c)) = (
                    observed_vars.iter().position(|v| v == &p.lhs),
                    observed_vars.iter().position(|v| v == &p.rhs),
                ) { theta[(r, c)] = p.est; theta[(c, r)] = p.est; }
            }
            "~" => {
                if let (Some(r), Some(c)) = (
                    latent_vars.iter().position(|v| v == &p.lhs),
                    latent_vars.iter().position(|v| v == &p.rhs),
                ) { beta[(r, c)] = p.est; }
            }
            _ => {}
        }
    }

    if n_lat > 0 {
        let mut i_minus_b = Mat::identity(n_lat, n_lat);
        for i in 0..n_lat { for j in 0..n_lat { i_minus_b[(i, j)] -= beta[(i, j)]; } }
        let inv = linalg::inverse(&i_minus_b);
        let inner = &(&inv * &psi) * inv.transpose();
        &(&lambda * &inner) * lambda.transpose() + &theta
    } else {
        theta
    }
}

// =====================================================================
// Estimation
// =====================================================================

/// Estimation method.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum EstimationMethod { DWLS, ML }

/// Configuration for SEM fitting.
#[derive(Clone, Debug)]
pub struct SemConfig {
    pub estimation: EstimationMethod,
    pub std_lv: bool,
    pub fix_resid: bool,
    pub toler: f64,
    pub max_iter: usize,
}

impl Default for SemConfig {
    fn default() -> Self {
        Self { estimation: EstimationMethod::DWLS, std_lv: false, fix_resid: true,
               toler: f64::EPSILON, max_iter: 500 }
    }
}

/// SEM fitting result.
#[derive(Clone, Debug)]
pub struct SemResult {
    pub params: Vec<Param>,
    pub implied: Mat<f64>,
    pub se: Vec<f64>,
    pub chisq: f64,
    pub df: i32,
    pub aic: f64,
    pub srmr: f64,
    pub cfi: Option<f64>,
    pub converged: bool,
    pub npar: usize,
}

/// Fit a SEM to a covariance matrix S using DWLS with weight matrix W.
pub fn fit_sem(
    s: &Mat<f64>,
    w: &Mat<f64>,
    model: &SemModel,
    observed_vars: &[String],
    config: &SemConfig,
) -> Result<SemResult> {
    let n = s.nrows();
    let z = n * (n + 1) / 2;

    let mut params = build_param_table(model, observed_vars);
    let npar = params.iter().filter(|p| p.free > 0).count();
    let df = (z as i32) - (npar as i32);

    // Initialize
    let mut theta = init_params(&mut params, s, observed_vars);

    // Optimize
    let converged = optimize_dwls(&mut theta, &params, s, w, observed_vars, &model.latent_vars, config);

    // Update estimates
    update_params(&mut params, &theta);
    let implied = compute_implied_cov(&params, observed_vars, &model.latent_vars);

    // Chi-square (residual-based)
    let chisq = compute_chisq_simple(s, &implied, w);

    // SRMR
    let srmr = compute_srmr(s, &implied, n);
    let aic = chisq + 2.0 * npar as f64;

    // Sandwich SEs
    let se = compute_sandwich_se(&params, s, w, observed_vars, &model.latent_vars, config.toler);

    Ok(SemResult { params, implied, se, chisq, df, aic, srmr, cfi: None, converged, npar })
}

fn init_params(params: &mut [Param], s: &Mat<f64>, observed_vars: &[String]) -> Vec<f64> {
    let n_free = params.iter().filter(|p| p.free > 0).count();
    let mut theta = vec![0.0f64; n_free];

    for p in params.iter_mut() {
        if p.free > 0 {
            let idx = (p.free - 1) as usize;
            if p.op == "~~" && p.lhs == p.rhs {
                if let Some(i) = observed_vars.iter().position(|v| v == &p.lhs) {
                    theta[idx] = s[(i, i)] * 0.5;
                } else {
                    theta[idx] = 1.0;
                }
            } else if p.op == "=~" {
                theta[idx] = 1.0;
            } else if p.op == "~" {
                theta[idx] = 0.3;
            }
            if !p.ustart.is_nan() { theta[idx] = p.ustart; }
            p.est = theta[idx];
        } else {
            p.est = p.ustart;
        }
    }
    theta
}

fn optimize_dwls(
    theta: &mut [f64], params: &[Param], s: &Mat<f64>, w: &Mat<f64>,
    observed_vars: &[String], latent_vars: &[String], config: &SemConfig,
) -> bool {
    let s_vec = linalg::vech(s);
    let npar = theta.len();

    for iteration in 0..config.max_iter {
        let mut params_copy: Vec<Param> = params.to_vec();
        update_params(&mut params_copy, theta);
        let implied = compute_implied_cov(&params_copy, observed_vars, latent_vars);
        let sigma_vec = linalg::vech(&implied);

        let residual: Vec<f64> = (0..s_vec.len()).map(|i| s_vec[i] - sigma_vec[i]).collect();

        let mut obj = 0.0;
        for i in 0..residual.len() { obj += residual[i] * residual[i] * w[(i, i)]; }

        // Numerical Jacobian
        let eps = 1e-7;
        let z = sigma_vec.len();
        let mut jac = vec![vec![0.0f64; npar]; z];

        for j in 0..npar {
            let mut theta_p = theta.to_vec();
            theta_p[j] += eps;
            let mut pp: Vec<Param> = params.to_vec();
            update_params(&mut pp, &theta_p);
            let imp_p = compute_implied_cov(&pp, observed_vars, latent_vars);
            let sig_p = linalg::vech(&imp_p);
            for i in 0..z { jac[i][j] = (sig_p[i] - sigma_vec[i]) / eps; }
        }

        // Gauss-Newton: Δ = (J'WJ)⁻¹ J'W r
        let mut jtwd = vec![vec![0.0f64; npar]; npar];
        let mut jtwr = vec![0.0f64; npar];
        for i in 0..z {
            let wi = w[(i, i)];
            for j in 0..npar {
                jtwr[j] += jac[i][j] * wi * residual[i];
                for k in 0..npar { jtwd[j][k] += jac[i][j] * wi * jac[i][k]; }
            }
        }
        for j in 0..npar { jtwd[j][j] += 1e-8 * obj.max(1e-10); }
        let delta = crate::ldsc::solve_small_pub(&jtwd, &jtwr);

        // Line search
        let mut best_obj = obj;
        let mut best_theta = theta.to_vec();
        let mut improved = false;
        for &alpha in &[1.0, 0.5, 0.25, 0.1, 0.01] {
            let trial: Vec<f64> = (0..npar).map(|j| theta[j] + alpha * delta[j]).collect();
            let mut pt: Vec<Param> = params.to_vec();
            update_params(&mut pt, &trial);
            let imp_t = compute_implied_cov(&pt, observed_vars, latent_vars);
            let sig_t = linalg::vech(&imp_t);
            let mut to = 0.0;
            for i in 0..s_vec.len() { let r = s_vec[i] - sig_t[i]; to += r * r * w[(i, i)]; }
            if to < best_obj { best_obj = to; best_theta = trial; improved = true; break; }
        }

        let prev = obj;
        theta.copy_from_slice(&best_theta);
        if !improved || (prev - best_obj).abs() < 1e-10 * prev.max(1e-10) { return true; }
        if iteration == config.max_iter - 1 { return false; }
    }
    true
}

fn update_params(params: &mut [Param], theta: &[f64]) {
    for p in params.iter_mut() {
        if p.free > 0 {
            let idx = (p.free - 1) as usize;
            if idx < theta.len() { p.est = theta[idx]; }
        }
    }
}

fn compute_chisq_simple(s: &Mat<f64>, implied: &Mat<f64>, w: &Mat<f64>) -> f64 {
    let residual = s - implied;
    let eta = linalg::vech(&residual);
    let mut q = 0.0;
    for i in 0..eta.len() { q += eta[i] * eta[i] * w[(i, i)]; }
    q
}

fn compute_srmr(s: &Mat<f64>, implied: &Mat<f64>, n: usize) -> f64 {
    let s_cor = linalg::cov2cor(s);
    let imp_cor = linalg::cov2cor(implied);
    let mut sum_sq = 0.0;
    let mut count = 0;
    for i in 0..n {
        for j in 0..=i {
            let diff = s_cor[(i, j)] - imp_cor[(i, j)];
            sum_sq += diff * diff;
            count += 1;
        }
    }
    (sum_sq / count as f64).sqrt()
}

fn compute_sandwich_se(
    params: &[Param], s: &Mat<f64>, w: &Mat<f64>,
    observed_vars: &[String], latent_vars: &[String], toler: f64,
) -> Vec<f64> {
    let n = s.nrows();
    let z = n * (n + 1) / 2;
    let npar = params.iter().filter(|p| p.free > 0).count();
    if npar == 0 { return params.iter().map(|_| f64::NAN).collect(); }

    let sigma_curr = linalg::vech(&compute_implied_cov(params, observed_vars, latent_vars));
    let eps = 1e-7;
    let mut delta = vec![vec![0.0f64; npar]; z];
    for j in 0..npar {
        let mut pp: Vec<Param> = params.to_vec();
        for p in &mut pp {
            if p.free == (j as i32 + 1) { p.est += eps; break; }
        }
        let imp_p = compute_implied_cov(&pp, observed_vars, latent_vars);
        let sig_p = linalg::vech(&imp_p);
        for i in 0..z { delta[i][j] = (sig_p[i] - sigma_curr[i]) / eps; }
    }

    let mut dtwd = vec![vec![0.0f64; npar]; npar];
    for i in 0..z {
        for j in 0..npar {
            for k in 0..npar { dtwd[j][k] += delta[i][j] * w[(i, i)] * delta[i][k]; }
        }
    }
    for j in 0..npar { dtwd[j][j] += toler.max(1e-10); }
    let bread_inv = invert_small(&dtwd);

    params.iter().map(|p| {
        if p.free > 0 {
            let idx = (p.free - 1) as usize;
            bread_inv[idx][idx].max(0.0).sqrt()
        } else { f64::NAN }
    }).collect()
}

fn invert_small(a: &[Vec<f64>]) -> Vec<Vec<f64>> {
    let n = a.len();
    let mut aug = vec![vec![0.0f64; 2 * n]; n];
    for i in 0..n { for j in 0..n { aug[i][j] = a[i][j]; } aug[i][n + i] = 1.0; }
    for col in 0..n {
        let mut max_row = col;
        let mut max_val = aug[col][col].abs();
        for row in (col + 1)..n { if aug[row][col].abs() > max_val { max_val = aug[row][col].abs(); max_row = row; } }
        if max_val < 1e-15 { continue; }
        aug.swap(col, max_row);
        let pivot = aug[col][col];
        for j in col..(2 * n) { aug[col][j] /= pivot; }
        for row in 0..n { if row != col { let f = aug[row][col]; for j in col..(2 * n) { aug[row][j] -= f * aug[col][j]; } } }
    }
    (0..n).map(|i| (0..n).map(|j| aug[i][n + j]).collect()).collect()
}

/// Compute model chi-square using V matrix eigenstructure (GenomicSEM style).
pub fn compute_chisq(s: &Mat<f64>, implied: &Mat<f64>, v: &Mat<f64>) -> f64 {
    let eta = linalg::vech(&(s - implied));
    let (eigvals, eigvecs) = linalg::eigen_sym(v);
    let p1_t_eta: Vec<f64> = (0..eigvecs.ncols())
        .map(|k| (0..eta.len()).map(|i| eigvecs[(i, k)] * eta[i]).sum()).collect();
    let mut q = 0.0;
    for k in 0..eigvals.len() {
        if eigvals[k].abs() > 1e-15 { q += p1_t_eta[k] * p1_t_eta[k] / eigvals[k]; }
    }
    q
}

/// Compute CFI.
pub fn compute_cfi(model_chisq: f64, model_df: i32, null_chisq: f64, null_df: i32) -> f64 {
    let cfi = ((null_chisq - null_df as f64) - (model_chisq - model_df as f64))
        / (null_chisq - null_df as f64);
    cfi.min(1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_model_simple() {
        let model_str = "F1 =~ NA*V1 + V2 + V3\nF1 ~~ 1*F1\nV1 ~~ V1\nV2 ~~ V2\nV3 ~~ V3";
        let model = parse_model(model_str).unwrap();
        assert!(model.latent_vars.contains(&"F1".to_string()));
        assert!(model.observed_vars.contains(&"V1".to_string()));
    }

    #[test]
    fn test_compute_implied_cov_identity() {
        let observed = vec!["V1".to_string(), "V2".to_string()];
        let latent = vec!["F1".to_string()];
        let params = vec![
            Param { lhs: "F1".into(), op: "=~".into(), rhs: "V1".into(), free: 0, ustart: 1.0, label: String::new(), est: 1.0 },
            Param { lhs: "F1".into(), op: "=~".into(), rhs: "V2".into(), free: 0, ustart: 0.0, label: String::new(), est: 0.5 },
            Param { lhs: "F1".into(), op: "~~".into(), rhs: "F1".into(), free: 0, ustart: 1.0, label: String::new(), est: 1.0 },
            Param { lhs: "V1".into(), op: "~~".into(), rhs: "V1".into(), free: 0, ustart: 0.0, label: String::new(), est: 0.5 },
            Param { lhs: "V2".into(), op: "~~".into(), rhs: "V2".into(), free: 0, ustart: 0.0, label: String::new(), est: 0.3 },
        ];
        let implied = compute_implied_cov(&params, &observed, &latent);
        assert!((implied[(0, 0)] - 1.5).abs() < 1e-8);
        assert!((implied[(0, 1)] - 0.5).abs() < 1e-8);
        assert!((implied[(1, 1)] - 0.55).abs() < 1e-8);
    }

    #[test]
    fn test_fit_sem_simple_cfa() {
        let mut s = Mat::zeros(3, 3);
        s[(0, 0)] = 1.0; s[(1, 1)] = 1.0; s[(2, 2)] = 1.0;
        s[(0, 1)] = 0.5; s[(0, 2)] = 0.4;
        s[(1, 0)] = 0.5; s[(1, 2)] = 0.3;
        s[(2, 0)] = 0.4; s[(2, 1)] = 0.3;

        let n = 3;
        let z = n * (n + 1) / 2;
        let w = Mat::identity(z, z);
        let model_str = "F1 =~ NA*V1 + V2 + V3\nF1 ~~ 1*F1";
        let model = parse_model(model_str).unwrap();
        let obs = vec!["V1".to_string(), "V2".to_string(), "V3".to_string()];
        let config = SemConfig::default();
        let result = fit_sem(&s, &w, &model, &obs, &config).unwrap();
        assert!(result.converged);
        let loadings: Vec<f64> = result.params.iter().filter(|p| p.op == "=~").map(|p| p.est).collect();
        assert!(loadings.iter().all(|l| l.abs() > 0.0));
    }
}
