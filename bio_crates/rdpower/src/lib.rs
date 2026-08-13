//! `rdpower`: Rust port of the R `rdpower` package.
//!
//! Power, sample size, and minimum detectable effect (MDE) calculations
//! for regression discontinuity (RD) designs using local polynomial methods.
//!
//! Faithful port of Cattaneo, Titiunik & Vázquez-Bare (2019),
//! "Power Calculations for Regression Discontinuity Designs",
//! *Stata Journal* 19(1): 210–245.

use statrs::distribution::{Continuous, ContinuousCDF, Normal};
use thiserror::Error;

pub use rdrobust::{Kernel, RdRobustConfig, RdRobustOutput, rdrobust};

// =====================================================================
// Error
// =====================================================================

#[derive(Debug, Error)]
pub enum RdPowerError {
    #[error("{0}")]
    Msg(String),
    #[error("rdrobust estimation failed: {0}")]
    RdRobust(#[from] rdrobust::RdRobustError),
    #[error("not enough information to calculate power without data")]
    InsufficientInfo,
    #[error("Newton-Raphson did not converge after {0} iterations")]
    NoConverge(usize),
}

// =====================================================================
// Normal distribution helpers
// =====================================================================

fn normal() -> Normal {
    Normal::new(0.0, 1.0).unwrap()
}

fn pnorm(x: f64) -> f64 {
    normal().cdf(x)
}

fn dnorm(x: f64) -> f64 {
    normal().pdf(x)
}

fn qnorm(p: f64) -> f64 {
    normal().inverse_cdf(p)
}

// =====================================================================
// Power functions (faithful port of rdpower_fun.R)
// =====================================================================

/// Power function: `1 - Φ(√n·τ/s̃ + z) + Φ(√n·τ/s̃ - z)`.
fn powerfun(n: f64, tau: f64, stilde: f64, z: f64) -> f64 {
    let x = n.sqrt() * tau / stilde;
    1.0 - pnorm(x + z) + pnorm(x - z)
}

/// Derivative of power function w.r.t. n.
fn powerfun_dot_n(n: f64, tau: f64, stilde: f64, z: f64) -> f64 {
    let x = n.sqrt() * tau / stilde;
    (dnorm(x - z) - dnorm(x + z)) * tau / (2.0 * stilde * n.sqrt())
}

/// Derivative of power function w.r.t. tau.
fn powerfun_dot_tau(n: f64, tau: f64, stilde: f64, z: f64) -> f64 {
    let x = n.sqrt() * tau / stilde;
    (dnorm(x - z) - dnorm(x + z)) * n.sqrt() / stilde
}

// =====================================================================
// Newton-Raphson solvers
// =====================================================================

/// Newton-Raphson to find sample size n achieving power `beta`.
///
/// Faithful port of `rdpower.powerNR`.
struct NrResult {
    m: usize, // ceiling of solution
    iter: usize,
    powercheck: f64,
}

fn power_nr(x0: f64, tau: f64, stilde: f64, z: f64, beta: f64) -> NrResult {
    let eps = 1e-12;
    let max_iter = 10000;
    let mut tol = 1.0_f64;
    let mut iter = 0;
    let mut x0 = x0;

    while tol > eps && iter < max_iter {
        iter += 1;
        let mut k = 1.0;

        // Check if derivative is too small
        let mut dot0 = powerfun_dot_n(x0, tau, stilde, z);
        let mut power0 = powerfun(x0, tau, stilde, z);

        let mut inner_count = 0;
        while dot0 < 0.00001 && inner_count < 100 {
            x0 = if power0 <= beta { 1.2 * x0 } else { 0.8 * x0 };
            inner_count += 1;
            iter += 1;
            dot0 = powerfun_dot_n(x0, tau, stilde, z);
            power0 = powerfun(x0, tau, stilde, z);
        }
        if inner_count >= 100 {
            break;
        }

        let mut x1 = x0 - (power0 - beta) / dot0;

        // Check if x1 is negative or too small
        let mut x1_iter = 0;
        while x1 < 2.0 && x1_iter < 100 {
            x1 = x0 - k * (power0 - beta) / dot0;
            k /= 2.0;
            x1_iter += 1;
            iter += 1;
        }

        tol = (powerfun(x1, tau, stilde, z) - beta).abs();
        x0 = x1;
    }

    let b = powerfun(x0, tau, stilde, z);
    NrResult {
        m: x0.ceil() as usize,
        iter,
        powercheck: b,
    }
}

/// Newton-Raphson to find MDE (tau) achieving power `beta`.
///
/// Faithful port of `rdpower.powerNR.mde`.
fn power_nr_mde(n: f64, tau0: f64, stilde: f64, z: f64, beta: f64) -> NrMdeResult {
    // Use a practical tolerance (R uses .Machine$double.eps but this can cause
    // non-convergence for large samples where the MDE is very small).
    let eps = 1e-12;
    let max_iter = 10000;
    let mut tol = 1.0_f64;
    let mut iter = 0;
    let mut tau0 = tau0;

    while tol > eps && iter < max_iter {
        iter += 1;

        let mut dot0 = powerfun_dot_tau(n, tau0, stilde, z);
        let mut power0 = powerfun(n, tau0, stilde, z);

        let mut inner_iter = 0;
        while dot0 < 0.00001 && inner_iter < 100 {
            // For MDE, tau should be positive. If tau0 went negative,
            // take absolute value before scaling.
            tau0 = tau0.abs();
            tau0 = if power0 <= beta {
                1.5 * tau0
            } else {
                0.5 * tau0
            };
            inner_iter += 1;
            iter += 1;
            dot0 = powerfun_dot_tau(n, tau0, stilde, z);
            power0 = powerfun(n, tau0, stilde, z);
        }
        if inner_iter >= 100 {
            break; // Can't find a region with non-zero derivative; accept current tau0.
        }

        let tau1 = tau0 - (power0 - beta) / dot0;
        tol = (powerfun(n, tau1, stilde, z) - beta).abs();
        tau0 = tau1;
    }

    let b = powerfun(n, tau0, stilde, z);
    NrMdeResult {
        mde: tau0,
        iter,
        powercheck: b,
    }
}

struct NrMdeResult {
    mde: f64,
    iter: usize,
    powercheck: f64,
}

// =====================================================================
// Sample statistics helper
// =====================================================================

fn sd(v: &[f64]) -> f64 {
    let n = v.len() as f64;
    if n < 2.0 {
        return 0.0;
    }
    let mean = v.iter().sum::<f64>() / n;
    let var = v.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / (n - 1.0);
    var.sqrt()
}

// =====================================================================
// rdpower: Power calculations for RD designs
// =====================================================================

/// Configuration for `rdpower()`.
#[derive(Clone, Debug)]
pub struct RdPowerConfig {
    /// Outcome variable.
    pub y: Vec<f64>,
    /// Running variable.
    pub r: Vec<f64>,
    /// RD cutoff.
    pub cutoff: f64,
    /// Treatment effect under alternative. Default: 0.5 * sd(Y left of cutoff).
    pub tau: Option<f64>,
    /// Significance level. Default 0.05.
    pub alpha: f64,
    /// Sample sizes at each side (left, right). Default: from bandwidth.
    pub sampsi: Option<[usize; 2]>,
    /// Bandwidths at each side (left, right). Default: from rdrobust.
    pub samph: Option<[f64; 2]>,
    /// Bias (left, right). Default: estimated from rdrobust.
    pub bias: Option<[f64; 2]>,
    /// Variance (left, right). Default: estimated from rdrobust.
    pub variance: Option<[f64; 2]>,
    // rdrobust options
    pub p: usize,
    pub deriv: usize,
    pub kernel: Kernel,
    pub bwselect: String,
    pub vce: String,
    pub cluster: Option<Vec<f64>>,
    pub covs: Option<faer::Mat<f64>>,
    pub level: f64,
}

impl Default for RdPowerConfig {
    fn default() -> Self {
        Self {
            y: vec![],
            r: vec![],
            cutoff: 0.0,
            tau: None,
            alpha: 0.05,
            sampsi: None,
            samph: None,
            bias: None,
            variance: None,
            p: 1,
            deriv: 0,
            kernel: Kernel::Triangular,
            bwselect: "mserd".into(),
            vce: "nn".into(),
            cluster: None,
            covs: None,
            level: 95.0,
        }
    }
}

/// Output of `rdpower()`.
#[derive(Clone, Debug)]
pub struct RdPowerResult {
    pub power_rbc: f64,
    pub se_rbc: f64,
    pub power_conv: f64,
    pub se_conv: f64,
    pub sampsi_r: usize,
    pub sampsi_l: usize,
    pub samph_r: f64,
    pub samph_l: f64,
    pub n_r: usize,
    pub n_l: usize,
    pub tau: f64,
    pub alpha: f64,
    pub bias_r: f64,
    pub bias_l: f64,
    pub vr_rb: f64,
    pub vl_rb: f64,
    /// Power at tau * {0, 0.2, 0.5, 0.8, 1.0} (RBC).
    pub power_rbc_list: [f64; 5],
    /// Power at tau * {0, 0.2, 0.5, 0.8, 1.0} (conventional).
    pub power_conv_list: [f64; 5],
    pub size_dist: f64,
}

/// Perform power calculations for RD designs.
///
/// Faithful port of `rdpower::rdpower()`.
pub fn rdpower(cfg: &RdPowerConfig) -> Result<RdPowerResult, RdPowerError> {
    let p = cfg.p;
    let deriv = cfg.deriv;
    let cutoff = cfg.cutoff;
    let alpha = cfg.alpha;
    let z = qnorm(1.0 - alpha / 2.0);

    // Run rdrobust to get bias, variance, bandwidths
    let rd_cfg = RdRobustConfig {
        y: cfg.y.clone(),
        x: cfg.r.clone(),
        c: cutoff,
        deriv,
        p,
        q: p + 1,
        kernel: cfg.kernel,
        bwselect: cfg.bwselect.clone(),
        vce: cfg.vce.clone(),
        cluster: cfg.cluster.clone(),
        covs: cfg.covs.clone(),
        level: cfg.level,
        ..Default::default()
    };
    let aux = rdrobust(&rd_cfg)?;

    let h_l = aux.h_l;
    let h_r = aux.h_r;

    // Bias normalization
    let (bias_l, bias_r) = match cfg.bias {
        Some(b) => (b[0], b[1]),
        None => (
            aux.bias_l / h_l.powi((1 + p - deriv) as i32),
            aux.bias_r / h_r.powi((1 + p - deriv) as i32),
        ),
    };

    // Variance extraction
    let (vl_rb, vr_rb, vl_cl, vr_cl) = match cfg.variance {
        Some(v) => (v[0], v[1], v[0], v[1]),
        None => {
            let pos = 1 + deriv; // 1-based → 0-based index = deriv
            let n = aux.n_l + aux.n_r;
            let vl_rb = n as f64 * h_l.powi((1 + 2 * deriv) as i32) * aux.v_rb_l[(deriv, deriv)];
            let vr_rb = n as f64 * h_r.powi((1 + 2 * deriv) as i32) * aux.v_rb_r[(deriv, deriv)];
            let vl_cl = n as f64 * h_l.powi((1 + 2 * deriv) as i32) * aux.v_cl_l[(deriv, deriv)];
            let vr_cl = n as f64 * h_r.powi((1 + 2 * deriv) as i32) * aux.v_cl_r[(deriv, deriv)];
            (vl_rb, vr_rb, vl_cl, vr_cl)
        }
    };

    // Default bandwidths
    let (hnew_l, hnew_r) = match cfg.samph {
        Some(h) => (h[0], h[1]),
        None => (h_l, h_r),
    };

    // Sample sizes inside bandwidth
    let nplus: usize = cfg
        .r
        .iter()
        .filter(|r| **r >= cutoff && r.is_finite())
        .count();
    let nminus: usize = cfg
        .r
        .iter()
        .filter(|r| **r < cutoff && r.is_finite())
        .count();
    let n_hnew_r: usize = cfg
        .r
        .iter()
        .filter(|r| **r >= cutoff && **r <= cutoff + hnew_r && r.is_finite())
        .count();
    let n_hnew_l: usize = cfg
        .r
        .iter()
        .filter(|r| **r < cutoff && **r >= cutoff - hnew_l && r.is_finite())
        .count();

    // Default sampsi
    let (ntilde_l, ntilde_r) = match cfg.sampsi {
        Some(s) => (s[0], s[1]),
        None => (n_hnew_l, n_hnew_r),
    };

    // Default tau
    let tau = match cfg.tau {
        Some(t) => t,
        None => {
            let left_of_cutoff: Vec<f64> = cfg
                .y
                .iter()
                .zip(&cfg.r)
                .filter(|(_, r)| **r >= cutoff - hnew_l && **r < cutoff)
                .map(|(y, _)| *y)
                .collect();
            0.5 * sd(&left_of_cutoff)
        }
    };

    // ntilde adjustment
    let ntilde = nplus as f64 * (ntilde_r as f64 / n_hnew_r as f64)
        + nminus as f64 * (ntilde_l as f64 / n_hnew_l as f64);

    // Variance adjustment
    let v_rbc = vl_rb / (ntilde * hnew_l.powi((1 + 2 * deriv) as i32))
        + vr_rb / (ntilde * hnew_r.powi((1 + 2 * deriv) as i32));
    let se_rbc = v_rbc.sqrt();

    let v_conv = vl_cl / (ntilde * hnew_l.powi((1 + 2 * deriv) as i32))
        + vr_cl / (ntilde * hnew_r.powi((1 + 2 * deriv) as i32));
    let se_conv = v_conv.sqrt();

    // Bias adjustment
    let bias =
        bias_r * hnew_r.powi((1 + p - deriv) as i32) + bias_l * hnew_l.powi((1 + p - deriv) as i32);

    // Power calculation
    let power_rbc = 1.0 - pnorm(tau / se_rbc + z) + pnorm(tau / se_rbc - z);
    let power_conv = 1.0 - pnorm((tau + bias) / se_conv + z) + pnorm((tau + bias) / se_conv - z);

    // Power at grid
    let te_grid = [0.0, 0.2, 0.5, 0.8, 1.0];
    let power_rbc_list: [f64; 5] = te_grid.map(|g| {
        let t = tau * g;
        1.0 - pnorm(t / se_rbc + z) + pnorm(t / se_rbc - z)
    });
    let power_conv_list: [f64; 5] = te_grid.map(|g| {
        let t = tau * g;
        1.0 - pnorm((t + bias) / se_conv + z) + pnorm((t + bias) / se_conv - z)
    });

    let size_dist = power_conv_list[4] - alpha;

    Ok(RdPowerResult {
        power_rbc,
        se_rbc,
        power_conv,
        se_conv,
        sampsi_r: ntilde_r,
        sampsi_l: ntilde_l,
        samph_r: hnew_r,
        samph_l: hnew_l,
        n_r: nplus,
        n_l: nminus,
        tau,
        alpha,
        bias_r,
        bias_l,
        vr_rb,
        vl_rb,
        power_rbc_list,
        power_conv_list,
        size_dist,
    })
}

// =====================================================================
// rdsampsi: Sample size calculations for RD designs
// =====================================================================

/// Configuration for `rdsampsi()`.
#[derive(Clone, Debug)]
pub struct RdSampsiConfig {
    pub y: Vec<f64>,
    pub r: Vec<f64>,
    pub cutoff: f64,
    pub tau: Option<f64>,
    pub alpha: f64,
    pub beta: f64,
    pub samph: Option<[f64; 2]>,
    pub bias: Option<[f64; 2]>,
    pub variance: Option<[f64; 2]>,
    pub nratio: Option<f64>,
    pub init_cond: Option<f64>,
    pub p: usize,
    pub deriv: usize,
    pub kernel: Kernel,
    pub bwselect: String,
    pub vce: String,
    pub cluster: Option<Vec<f64>>,
}

impl Default for RdSampsiConfig {
    fn default() -> Self {
        Self {
            y: vec![],
            r: vec![],
            cutoff: 0.0,
            tau: None,
            alpha: 0.05,
            beta: 0.8,
            samph: None,
            bias: None,
            variance: None,
            nratio: None,
            init_cond: None,
            p: 1,
            deriv: 0,
            kernel: Kernel::Triangular,
            bwselect: "mserd".into(),
            vce: "nn".into(),
            cluster: None,
        }
    }
}

/// Output of `rdsampsi()`.
#[derive(Clone, Debug)]
pub struct RdSampsiResult {
    pub sampsi_h_tot: usize,
    pub sampsi_h_r: usize,
    pub sampsi_h_l: usize,
    pub sampsi_tot: usize,
    pub n_r: usize,
    pub n_l: usize,
    pub samph_r: f64,
    pub samph_l: f64,
    pub tau: f64,
    pub beta: f64,
    pub alpha: f64,
    pub init_cond: f64,
    pub no_iter: usize,
    pub sampsi_h_tot_cl: usize,
    pub sampsi_h_r_cl: usize,
    pub sampsi_h_l_cl: usize,
    pub sampsi_tot_cl: usize,
    pub bias_r: f64,
    pub bias_l: f64,
    pub var_r: f64,
    pub var_l: f64,
    pub var_r_cl: f64,
    pub var_l_cl: f64,
    pub nratio: f64,
    pub nratio_cl: f64,
    pub size_dist: f64,
}

/// Perform sample size calculations for RD designs.
///
/// Faithful port of `rdpower::rdsampsi()`.
pub fn rdsampsi(cfg: &RdSampsiConfig) -> Result<RdSampsiResult, RdPowerError> {
    let p = cfg.p;
    let deriv = cfg.deriv;
    let cutoff = cfg.cutoff;
    let alpha = cfg.alpha;
    let beta = cfg.beta;
    let z = qnorm(1.0 - alpha / 2.0);

    // Run rdrobust
    let rd_cfg = RdRobustConfig {
        y: cfg.y.clone(),
        x: cfg.r.clone(),
        c: cutoff,
        deriv,
        p,
        q: p + 1,
        kernel: cfg.kernel,
        bwselect: cfg.bwselect.clone(),
        vce: cfg.vce.clone(),
        cluster: cfg.cluster.clone(),
        ..Default::default()
    };
    let aux = rdrobust(&rd_cfg)?;

    let h_l = aux.h_l;
    let h_r = aux.h_r;

    // Bias
    let (bias_l, bias_r) = match cfg.bias {
        Some(b) => (b[0], b[1]),
        None => (
            aux.bias_l / h_l.powi((1 + p - deriv) as i32),
            aux.bias_r / h_r.powi((1 + p - deriv) as i32),
        ),
    };

    // Variance
    let (vl, vr, vl_cl, vr_cl) = match cfg.variance {
        Some(v) => (v[0], v[1], v[0], v[1]),
        None => {
            let n = aux.n_l + aux.n_r;
            let vl = n as f64 * h_l.powi((1 + 2 * deriv) as i32) * aux.v_rb_l[(deriv, deriv)];
            let vr = n as f64 * h_r.powi((1 + 2 * deriv) as i32) * aux.v_rb_r[(deriv, deriv)];
            let vl_cl = n as f64 * h_l.powi((1 + 2 * deriv) as i32) * aux.v_cl_l[(deriv, deriv)];
            let vr_cl = n as f64 * h_r.powi((1 + 2 * deriv) as i32) * aux.v_cl_r[(deriv, deriv)];
            (vl, vr, vl_cl, vr_cl)
        }
    };

    // Default bandwidths
    let (hnew_l, hnew_r) = match cfg.samph {
        Some(h) => (h[0], h[1]),
        None => (h_l, h_r),
    };

    // Bias adjustment
    let bias =
        bias_r * hnew_r.powi((1 + p - deriv) as i32) + bias_l * hnew_l.powi((1 + p - deriv) as i32);

    // Variance adjustment
    let v_rbc = vl / hnew_l.powi((1 + 2 * deriv) as i32) + vr / hnew_r.powi((1 + 2 * deriv) as i32);
    let stilde = v_rbc.sqrt();
    let v_cl =
        vl_cl / hnew_l.powi((1 + 2 * deriv) as i32) + vr_cl / hnew_r.powi((1 + 2 * deriv) as i32);
    let stilde_cl = v_cl.sqrt();

    // Default tau
    let tau = match cfg.tau {
        Some(t) => t,
        None => {
            let left: Vec<f64> = cfg
                .y
                .iter()
                .zip(&cfg.r)
                .filter(|(_, r)| **r >= cutoff - hnew_l && **r < cutoff)
                .map(|(y, _)| *y)
                .collect();
            0.5 * sd(&left)
        }
    };

    // Sample sizes
    let nplus: usize = cfg
        .r
        .iter()
        .filter(|r| **r >= cutoff && r.is_finite())
        .count();
    let nminus: usize = cfg
        .r
        .iter()
        .filter(|r| **r < cutoff && r.is_finite())
        .count();
    let n_hnew_r: usize = cfg
        .r
        .iter()
        .filter(|r| **r >= cutoff && **r <= cutoff + hnew_r && r.is_finite())
        .count();
    let n_hnew_l: usize = cfg
        .r
        .iter()
        .filter(|r| **r < cutoff && **r >= cutoff - hnew_l && r.is_finite())
        .count();

    // Initial condition
    let init_cond = cfg
        .init_cond
        .unwrap_or_else(|| cfg.r.iter().filter(|r| r.is_finite()).count() as f64);

    // Newton-Raphson for sample size
    let maux = power_nr(init_cond, tau, stilde, z, beta);
    let m = maux.m;

    let maux1 = power_nr(init_cond, tau + bias, stilde_cl, z, beta);
    let m_cl = maux1.m;

    // nratio
    let nratio = cfg
        .nratio
        .unwrap_or_else(|| vr.sqrt() / (vr.sqrt() + vl.sqrt()));
    let nratio_cl = cfg
        .nratio
        .unwrap_or_else(|| vr_cl.sqrt() / (vr_cl.sqrt() + vl_cl.sqrt()));

    // Adjust m for sample sizes
    let denom =
        nratio * nplus as f64 / n_hnew_r as f64 + (1.0 - nratio) * nminus as f64 / n_hnew_l as f64;
    let denom_cl = nratio_cl * nplus as f64 / n_hnew_r as f64
        + (1.0 - nratio_cl) * nminus as f64 / n_hnew_l as f64;

    let m_f = m as f64 / denom;
    let mr = (m_f * nratio).ceil() as usize;
    let ml = (m_f * (1.0 - nratio)).ceil() as usize;
    let m_total = ml + mr;

    let m_cl_f = m_cl as f64 / denom_cl;
    let mr_cl = (m_cl_f * nratio_cl).ceil() as usize;
    let ml_cl = (m_cl_f * (1.0 - nratio_cl)).ceil() as usize;
    let m_cl_total = ml_cl + mr_cl;

    // Size distortion
    let se_cl_aux = stilde_cl / (m_cl as f64).sqrt();
    let size_dist = 1.0 - pnorm(bias / se_cl_aux + z) + pnorm(bias / se_cl_aux - z);

    Ok(RdSampsiResult {
        sampsi_h_tot: m_total,
        sampsi_h_r: mr,
        sampsi_h_l: ml,
        sampsi_tot: m,
        n_r: nplus,
        n_l: nminus,
        samph_r: hnew_r,
        samph_l: hnew_l,
        tau,
        beta,
        alpha,
        init_cond,
        no_iter: maux.iter,
        sampsi_h_tot_cl: m_cl_total,
        sampsi_h_r_cl: mr_cl,
        sampsi_h_l_cl: ml_cl,
        sampsi_tot_cl: m_cl,
        bias_r,
        bias_l,
        var_r: vr,
        var_l: vl,
        var_r_cl: vr_cl,
        var_l_cl: vl_cl,
        nratio,
        nratio_cl,
        size_dist,
    })
}

// =====================================================================
// rdmde: Minimum detectable effect calculations for RD designs
// =====================================================================

/// Configuration for `rdmde()`.
#[derive(Clone, Debug)]
pub struct RdMdeConfig {
    pub y: Vec<f64>,
    pub r: Vec<f64>,
    pub cutoff: f64,
    pub alpha: f64,
    pub beta: f64,
    pub sampsi: Option<[usize; 2]>,
    pub samph: Option<[f64; 2]>,
    pub bias: Option<[f64; 2]>,
    pub variance: Option<[f64; 2]>,
    pub init_cond: Option<f64>,
    pub p: usize,
    pub deriv: usize,
    pub kernel: Kernel,
    pub bwselect: String,
    pub vce: String,
    pub cluster: Option<Vec<f64>>,
}

impl Default for RdMdeConfig {
    fn default() -> Self {
        Self {
            y: vec![],
            r: vec![],
            cutoff: 0.0,
            alpha: 0.05,
            beta: 0.8,
            sampsi: None,
            samph: None,
            bias: None,
            variance: None,
            init_cond: None,
            p: 1,
            deriv: 0,
            kernel: Kernel::Triangular,
            bwselect: "mserd".into(),
            vce: "nn".into(),
            cluster: None,
        }
    }
}

/// Output of `rdmde()`.
#[derive(Clone, Debug)]
pub struct RdMdeResult {
    pub mde: f64,
    pub se_rbc: f64,
    pub mde_conv: f64,
    pub se_conv: f64,
    pub sampsi_r: usize,
    pub sampsi_l: usize,
    pub samph_r: f64,
    pub samph_l: f64,
    pub n_r: usize,
    pub n_l: usize,
    pub alpha: f64,
    pub beta: f64,
    pub bias_r: f64,
    pub bias_l: f64,
    pub vr_rb: f64,
    pub vl_rb: f64,
    pub beta_list: [f64; 4],
    pub mde_rbc_list: [f64; 4],
    pub mde_conv_list: [f64; 4],
}

/// Perform MDE calculations for RD designs.
///
/// Faithful port of `rdpower::rdmde()`.
pub fn rdmde(cfg: &RdMdeConfig) -> Result<RdMdeResult, RdPowerError> {
    let p = cfg.p;
    let deriv = cfg.deriv;
    let cutoff = cfg.cutoff;
    let alpha = cfg.alpha;
    let beta = cfg.beta;
    let z = qnorm(1.0 - alpha / 2.0);

    // Run rdrobust
    let rd_cfg = RdRobustConfig {
        y: cfg.y.clone(),
        x: cfg.r.clone(),
        c: cutoff,
        deriv,
        p,
        q: p + 1,
        kernel: cfg.kernel,
        bwselect: cfg.bwselect.clone(),
        vce: cfg.vce.clone(),
        cluster: cfg.cluster.clone(),
        ..Default::default()
    };
    let aux = rdrobust(&rd_cfg)?;

    let h_l = aux.h_l;
    let h_r = aux.h_r;

    // Bias
    let (bias_l, bias_r) = match cfg.bias {
        Some(b) => (b[0], b[1]),
        None => (
            aux.bias_l / h_l.powi((1 + p - deriv) as i32),
            aux.bias_r / h_r.powi((1 + p - deriv) as i32),
        ),
    };

    // Variance
    let (vl_rb, vr_rb, vl_cl, vr_cl) = match cfg.variance {
        Some(v) => (v[0], v[1], v[0], v[1]),
        None => {
            let n = aux.n_l + aux.n_r;
            let vl_rb = n as f64 * h_l.powi((1 + 2 * deriv) as i32) * aux.v_rb_l[(deriv, deriv)];
            let vr_rb = n as f64 * h_r.powi((1 + 2 * deriv) as i32) * aux.v_rb_r[(deriv, deriv)];
            let vl_cl = n as f64 * h_l.powi((1 + 2 * deriv) as i32) * aux.v_cl_l[(deriv, deriv)];
            let vr_cl = n as f64 * h_r.powi((1 + 2 * deriv) as i32) * aux.v_cl_r[(deriv, deriv)];
            (vl_rb, vr_rb, vl_cl, vr_cl)
        }
    };

    // Default bandwidths
    let (hnew_l, hnew_r) = match cfg.samph {
        Some(h) => (h[0], h[1]),
        None => (h_l, h_r),
    };

    // Sample sizes
    let nplus: usize = cfg
        .r
        .iter()
        .filter(|r| **r >= cutoff && r.is_finite())
        .count();
    let nminus: usize = cfg
        .r
        .iter()
        .filter(|r| **r < cutoff && r.is_finite())
        .count();
    let n_hnew_r: usize = cfg
        .r
        .iter()
        .filter(|r| **r >= cutoff && **r <= cutoff + hnew_r && r.is_finite())
        .count();
    let n_hnew_l: usize = cfg
        .r
        .iter()
        .filter(|r| **r < cutoff && **r >= cutoff - hnew_l && r.is_finite())
        .count();

    // Default sampsi
    let (ntilde_l, ntilde_r) = match cfg.sampsi {
        Some(s) => (s[0], s[1]),
        None => (n_hnew_l, n_hnew_r),
    };

    let ntilde = nplus as f64 * (ntilde_r as f64 / n_hnew_r as f64)
        + nminus as f64 * (ntilde_l as f64 / n_hnew_l as f64);

    // Variance
    let v_rbc =
        vl_rb / hnew_l.powi((1 + 2 * deriv) as i32) + vr_rb / hnew_r.powi((1 + 2 * deriv) as i32);
    let se_rbc = v_rbc.sqrt();

    let v_conv =
        vl_cl / hnew_l.powi((1 + 2 * deriv) as i32) + vr_cl / hnew_r.powi((1 + 2 * deriv) as i32);
    let se_conv = v_conv.sqrt();

    // Bias adjustment
    let bias =
        bias_r * hnew_r.powi((1 + p - deriv) as i32) + bias_l * hnew_l.powi((1 + p - deriv) as i32);

    // Initial condition for Newton-Raphson
    let tau0 = match cfg.init_cond {
        Some(t) => t,
        None => {
            let below: Vec<f64> = cfg
                .y
                .iter()
                .zip(&cfg.r)
                .filter(|(_, r)| **r < cutoff)
                .map(|(y, _)| *y)
                .collect();
            0.2 * sd(&below)
        }
    };

    // MDE via Newton-Raphson
    let mde_aux = power_nr_mde(ntilde, tau0, se_rbc, z, beta);
    let mde = mde_aux.mde;

    // MDE at different beta values
    let beta_fracs = [-0.125, -0.0625, 0.0625, 0.125];
    let mut beta_list = [0.0; 4];
    let mut mde_rbc_list = [0.0; 4];
    let mut mde_conv_list = [0.0; 4];

    for (i, &frac) in beta_fracs.iter().enumerate() {
        let baux = beta * (1.0 + frac);
        beta_list[i] = baux;
        if baux < 1.0 {
            let r = power_nr_mde(ntilde, tau0, se_rbc, z, baux);
            mde_rbc_list[i] = r.mde;
        }
    }

    // Conventional MDE (ensure positive starting point)
    let tau0_conv = (tau0 + bias).max(tau0 * 0.1).abs();
    let mde_conv_aux = power_nr_mde(ntilde, tau0_conv, se_conv, z, beta);
    let mde_conv = mde_conv_aux.mde;

    for (i, &frac) in beta_fracs.iter().enumerate() {
        let baux = beta * (1.0 + frac);
        if baux < 1.0 {
            let r = power_nr_mde(ntilde, tau0_conv, se_conv, z, baux);
            mde_conv_list[i] = r.mde;
        }
    }

    Ok(RdMdeResult {
        mde,
        se_rbc,
        mde_conv,
        se_conv,
        sampsi_r: ntilde_r,
        sampsi_l: ntilde_l,
        samph_r: hnew_r,
        samph_l: hnew_l,
        n_r: nplus,
        n_l: nminus,
        alpha,
        beta,
        bias_r,
        bias_l,
        vr_rb,
        vl_rb,
        beta_list,
        mde_rbc_list,
        mde_conv_list,
    })
}
