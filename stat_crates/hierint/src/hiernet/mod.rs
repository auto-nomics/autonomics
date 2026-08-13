//! Placeholder — full implementation below.
pub mod admm;
pub mod cv;
pub mod ggdescent;
pub mod interactions;
pub mod prox;

use serde::{Deserialize, Serialize};

// ═══════════════════════════════════════════════════════════════════════
// Family
// ═══════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HierNetFamily {
    Gaussian,
    Logistic,
}

// ═══════════════════════════════════════════════════════════════════════
// Configuration
// ═══════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HierNetConfig {
    pub family: HierNetFamily,
    /// Use strong hierarchy (ADMM4). If false, weak hierarchy (single ggdescent).
    pub strong: bool,
    /// Include diagonal (x_j^2) terms.
    pub diagonal: bool,
    /// Elastic-net mixing: lam.l1 = lam*(1-delta), lam.l2 = lam*delta.
    pub delta: f64,
    pub n_lam: usize,
    pub flmin: f64,
    pub lamlist: Option<Vec<f64>>,
    /// ADMM parameter.
    pub rho: Option<f64>,
    pub niter: usize,
    pub sym_eps: f64,
    /// GG descent parameters.
    pub step: f64,
    pub maxiter: usize,
    pub backtrack: f64,
    pub tol: f64,
}

impl Default for HierNetConfig {
    fn default() -> Self {
        Self {
            family: HierNetFamily::Gaussian,
            strong: false,
            diagonal: true,
            delta: 1e-8,
            n_lam: 20,
            flmin: 0.01,
            lamlist: None,
            rho: None,
            niter: 100,
            sym_eps: 1e-3,
            step: 1.0,
            maxiter: 2000,
            backtrack: 0.2,
            tol: 1e-5,
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Coefficients
// ═══════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone)]
pub struct HierNetCoefs {
    /// Intercept (logistic only; for Gaussian it's folded into `my`)
    pub b0: f64,
    /// Positive part of main effects (length p)
    pub bp: Vec<f64>,
    /// Negative part of main effects (length p)
    pub bn: Vec<f64>,
    /// Interaction matrix (p×p, symmetric for strong hierarchy)
    pub th: Vec<f64>, // column-major p*p
    pub diagonal: bool,
}

impl HierNetCoefs {
    pub fn zeros(p: usize, diagonal: bool) -> Self {
        Self {
            b0: 0.0,
            bp: vec![0.0; p],
            bn: vec![0.0; p],
            th: vec![0.0; p * p],
            diagonal,
        }
    }

    pub fn main_effects(&self) -> Vec<f64> {
        self.bp.iter().zip(&self.bn).map(|(&p, &n)| p - n).collect()
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Fitted model (single lambda)
// ═══════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone)]
pub struct HierNetFit {
    pub coefs: HierNetCoefs,
    pub lam: f64,
    pub obj: f64,
    pub strong: bool,
    pub diagonal: bool,
    pub family: HierNetFamily,
    // Standardization attributes
    pub mx: Vec<f64>,  // centering for X
    pub sx: Vec<f64>,  // scaling for X (1.0 if not scaled)
    pub my: f64,       // centering for Y
    pub mzz: Vec<f64>, // centering for ZZ
    pub szz: Vec<f64>, // scaling for ZZ (1.0 if not scaled)
}

/// Path of fits over multiple lambdas
#[derive(Debug, Clone)]
pub struct HierNetPath {
    pub lamlist: Vec<f64>,
    pub fits: Vec<HierNetFit>,
    pub p: usize,
    pub family: HierNetFamily,
    pub strong: bool,
    pub diagonal: bool,
}

// ═══════════════════════════════════════════════════════════════════════
// Standardization
// ═══════════════════════════════════════════════════════════════════════

struct StandardizedX {
    data: Vec<f64>, // column-major n×p
    mx: Vec<f64>,
    sx: Vec<f64>,
}

fn standardize_x(x: &[f64], n: usize, p: usize, scale: bool) -> StandardizedX {
    let mut data = vec![0.0; n * p];
    let mut mx = vec![0.0; p];
    let mut sx = vec![1.0; p];

    for j in 0..p {
        let mut mean = 0.0;
        for i in 0..n {
            mean += x[j * n + i];
        }
        mean /= n as f64;
        mx[j] = mean;

        if scale {
            let mut var = 0.0;
            for i in 0..n {
                let d = x[j * n + i] - mean;
                var += d * d;
            }
            sx[j] = if var > 1e-30 { var.sqrt() } else { 1.0 };
        }

        for i in 0..n {
            data[j * n + i] = (x[j * n + i] - mx[j]) / sx[j];
        }
    }

    StandardizedX { data, mx, sx }
}

// ═══════════════════════════════════════════════════════════════════════
// Fit (single lambda)
// ═══════════════════════════════════════════════════════════════════════

/// Fit hierNet at a single lambda value.
///
/// - `x`: n×p column-major continuous predictor matrix
/// - `y`: response (length n). For logistic, must be {0, 1}.
/// - `config`: configuration
/// - `lam`: penalty parameter
/// - `warm`: optional warm-start coefficients
pub fn fit(
    x: &[f64],
    y: &[f64],
    config: &HierNetConfig,
    lam: f64,
    warm: Option<&HierNetCoefs>,
) -> crate::Result<HierNetFit> {
    let n = y.len();
    let p = x.len() / n;
    if p * n != x.len() {
        return Err(crate::HierIntError::DimMismatch(
            "x must be n×p column-major".into(),
        ));
    }

    // Standardize X
    let std_x = standardize_x(x, n, p, true);

    // Center Y (gaussian)
    let my = if config.family == HierNetFamily::Gaussian {
        let m = y.iter().sum::<f64>() / n as f64;
        m
    } else {
        0.0
    };
    let y_centered: Vec<f64> = y.iter().map(|&v| v - my).collect();

    // Compute interactions
    let zz_matrix = interactions::compute_interactions(&std_x.data, n, p, config.diagonal);
    let cp2 = if config.diagonal {
        p * (p - 1) / 2 + p
    } else {
        p * (p - 1) / 2
    };

    // Center ZZ
    let (zz, mzz, szz) = {
        let mut mzz = vec![0.0; cp2];
        let szz = vec![1.0; cp2];
        for j in 0..cp2 {
            let mut mean = 0.0;
            for i in 0..n {
                mean += zz_matrix[j * n + i];
            }
            mean /= n as f64;
            mzz[j] = mean;
        }
        let zz: Vec<f64> = (0..cp2 * n)
            .map(|idx| {
                let j = idx / n;
                let i = idx % n;
                zz_matrix[j * n + i] - mzz[j]
            })
            .collect();
        (zz, mzz, szz)
    };

    let lam_l1 = lam * (1.0 - config.delta);
    let lam_l2 = lam * config.delta;

    // Initialize coefficients
    let init = warm
        .cloned()
        .unwrap_or_else(|| HierNetCoefs::zeros(p, config.diagonal));

    let coefs = if config.family == HierNetFamily::Gaussian {
        if config.strong {
            let rho = config.rho.unwrap_or(n as f64);
            admm::admm4(
                &std_x.data,
                &zz,
                &y_centered,
                lam_l1,
                lam_l2,
                config.diagonal,
                rho,
                config.niter,
                config.sym_eps,
                config.step,
                config.maxiter,
                config.backtrack,
                config.tol,
                &init,
            )?
        } else {
            let v = vec![0.0; p * p];
            ggdescent::ggdescent(
                &std_x.data,
                n,
                p,
                &zz,
                config.diagonal,
                &y_centered,
                lam_l1,
                lam_l2,
                0.0,
                &v,
                config.step,
                config.backtrack,
                config.maxiter,
                config.tol,
                &init,
            )?
        }
    } else {
        // Logistic
        let mut init2 = init.clone();
        init2.b0 = 0.0;
        if config.strong {
            let rho = config.rho.unwrap_or(n as f64);
            admm::admm4_logistic(
                &std_x.data,
                &zz,
                y,
                lam_l1,
                lam_l2,
                config.diagonal,
                rho,
                config.niter,
                config.sym_eps,
                config.step,
                config.maxiter,
                config.backtrack,
                config.tol,
                &init2,
            )?
        } else {
            let v = vec![0.0; p * p];
            ggdescent::ggdescent_logistic(
                &std_x.data,
                n,
                p,
                &zz,
                config.diagonal,
                y,
                lam_l1,
                lam_l2,
                0.0,
                &v,
                config.step,
                config.backtrack,
                config.maxiter,
                config.tol,
                &init2,
            )?
        }
    };

    // Compute objective
    let obj = if config.family == HierNetFamily::Gaussian {
        compute_objective_gaussian(
            &std_x.data,
            &zz,
            &y_centered,
            &coefs,
            lam_l1,
            lam_l2,
            n,
            p,
            config.diagonal,
        )
    } else {
        compute_objective_logistic(
            &std_x.data,
            &zz,
            y,
            &coefs,
            lam_l1,
            lam_l2,
            n,
            p,
            config.diagonal,
        )
    };

    Ok(HierNetFit {
        coefs,
        lam,
        obj,
        strong: config.strong,
        diagonal: config.diagonal,
        family: config.family,
        mx: std_x.mx,
        sx: std_x.sx,
        my,
        mzz,
        szz,
    })
}

/// Fit a lambda path.
pub fn fit_path(x: &[f64], y: &[f64], config: &HierNetConfig) -> crate::Result<HierNetPath> {
    let n = y.len();
    let p = x.len() / n;

    // Lambda grid
    let lamlist = if let Some(ref ll) = config.lamlist {
        ll.clone()
    } else {
        let std_x = standardize_x(x, n, p, true);
        let my = y.iter().sum::<f64>() / n as f64;
        let yc: Vec<f64> = y.iter().map(|&v| v - my).collect();
        // maxlam = max|X^T y|
        let mut maxlam = 0.0f64;
        for j in 0..p {
            let mut dot = 0.0;
            for i in 0..n {
                dot += std_x.data[j * n + i] * yc[i];
            }
            maxlam = maxlam.max(dot.abs());
        }
        let minlam = maxlam * config.flmin;
        let log_max = maxlam.ln();
        let log_min = minlam.ln();
        (0..config.n_lam)
            .map(|k| {
                let frac = k as f64 / (config.n_lam - 1) as f64;
                (log_max + frac * (log_min - log_max)).exp()
            })
            .collect()
    };

    let n_lam = lamlist.len();
    let mut fits = Vec::with_capacity(n_lam);
    let mut warm: Option<HierNetCoefs> = None;

    for &lam in &lamlist {
        let fit = fit(x, y, config, lam, warm.as_ref())?;
        warm = Some(fit.coefs.clone());
        fits.push(fit);
    }

    Ok(HierNetPath {
        lamlist,
        fits,
        p,
        family: config.family,
        strong: config.strong,
        diagonal: config.diagonal,
    })
}

/// Predict from a single fit.
pub fn predict(fit: &HierNetFit, x_new: &[f64], n: usize) -> Vec<f64> {
    let p = fit.coefs.bp.len();

    // Standardize new X using training centering/scaling
    let x_std: Vec<f64> = (0..p)
        .flat_map(|j| (0..n).map(move |i| (x_new[j * n + i] - fit.mx[j]) / fit.sx[j]))
        .collect();

    // Compute interactions for new X
    let zz_raw = interactions::compute_interactions(&x_std, n, p, fit.diagonal);
    let cp2 = if fit.diagonal {
        p * (p - 1) / 2 + p
    } else {
        p * (p - 1) / 2
    };

    // Center using training mzz
    let zz: Vec<f64> = (0..cp2 * n)
        .map(|idx| {
            let j = idx / n;
            let i = idx % n;
            zz_raw[j * n + i] - fit.mzz[j]
        })
        .collect();

    let yhat = interactions::compute_yhat(
        &x_std,
        n,
        p,
        &zz,
        fit.diagonal,
        &fit.coefs.th,
        &fit.coefs.bp,
        &fit.coefs.bn,
    );

    if fit.family == HierNetFamily::Logistic {
        yhat.iter()
            .map(|&yh| 1.0 / (1.0 + (-(fit.coefs.b0 + yh)).exp()))
            .collect()
    } else {
        yhat.iter().map(|&yh| yh + fit.my).collect()
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Objective functions
// ═══════════════════════════════════════════════════════════════════════

fn compute_objective_gaussian(
    x: &[f64],
    zz: &[f64],
    y: &[f64],
    aa: &HierNetCoefs,
    lam_l1: f64,
    lam_l2: f64,
    n: usize,
    p: usize,
    diagonal: bool,
) -> f64 {
    let yhat = interactions::compute_yhat(x, n, p, zz, diagonal, &aa.th, &aa.bp, &aa.bn);
    let mut loss = 0.0;
    for i in 0..n {
        let r = y[i] - yhat[i];
        loss += r * r;
    }
    loss /= 2.0;

    // Penalty
    let mut pen = 0.0;
    for j in 0..p {
        pen += aa.bp[j] + aa.bn[j];
    }
    // Off-diagonal th terms (counted once due to symmetry)
    for j in 0..(p - 1) {
        for k in (j + 1)..p {
            pen += (aa.th[j + p * k] + aa.th[k + p * j]).abs() / 2.0;
        }
    }
    if diagonal {
        for j in 0..p {
            pen += aa.th[j + p * j].abs();
        }
    }
    pen *= lam_l1;

    // L2 penalty
    let mut l2 = 0.0;
    for j in 0..p {
        l2 += aa.bp[j] * aa.bp[j] + aa.bn[j] * aa.bn[j];
    }
    for jj in 0..(p * p) {
        l2 += aa.th[jj] * aa.th[jj];
    }
    pen += lam_l2 * l2;

    loss + pen
}

fn compute_objective_logistic(
    x: &[f64],
    zz: &[f64],
    y: &[f64],
    aa: &HierNetCoefs,
    lam_l1: f64,
    lam_l2: f64,
    n: usize,
    p: usize,
    diagonal: bool,
) -> f64 {
    let yhat = interactions::compute_yhat(x, n, p, zz, diagonal, &aa.th, &aa.bp, &aa.bn);
    let mut loss = 0.0;
    for i in 0..n {
        let z = aa.b0 + yhat[i];
        loss += (1.0 + (-(2.0 * y[i] - 1.0) * z).exp()).ln();
    }

    let mut pen = 0.0;
    for j in 0..p {
        pen += aa.bp[j] + aa.bn[j];
    }
    for j in 0..(p - 1) {
        for k in (j + 1)..p {
            pen += (aa.th[j + p * k] + aa.th[k + p * j]).abs() / 2.0;
        }
    }
    if diagonal {
        for j in 0..p {
            pen += aa.th[j + p * j].abs();
        }
    }
    pen *= lam_l1;

    let mut l2 = 0.0;
    for j in 0..p {
        l2 += aa.bp[j] * aa.bp[j] + aa.bn[j] * aa.bn[j];
    }
    for jj in 0..(p * p) {
        l2 += aa.th[jj] * aa.th[jj];
    }
    pen += lam_l2 * l2;

    loss + pen
}
