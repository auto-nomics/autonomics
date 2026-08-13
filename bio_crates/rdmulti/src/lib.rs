//! `rdmulti`: Rust port of the R `rdmulti` package.
//!
//! Point estimation and robust bias-corrected inference for multi-cutoff
//! and multi-score Regression Discontinuity (RD) designs.
//!
//! Faithful port of Cattaneo, Titiunik & Vázquez-Bare (2020),
//! "Analysis of Regression Discontinuity Designs with Multiple Cutoffs
//! or Multiple Scores", *Stata Journal*.

use statrs::distribution::{ContinuousCDF, Normal};
use thiserror::Error;

pub use rdrobust::{Kernel, RdRobustConfig, RdRobustOutput, rdrobust};

#[derive(Debug, Error)]
pub enum RdMultiError {
    #[error("{0}")]
    Msg(String),
    #[error("rdrobust failed: {0}")]
    RdRobust(#[from] rdrobust::RdRobustError),
}

// =====================================================================
// rdmc: Multi-cutoff RD
// =====================================================================

/// Per-cutoff results.
#[derive(Clone, Debug)]
pub struct CutoffResult {
    pub cutoff: f64,
    pub tau_cl: f64,
    pub tau_bc: f64,
    pub se_cl: f64,
    pub se_rb: f64,
    pub pv_rb: f64,
    pub pv_cl: f64,
    pub ci_rb: [f64; 2],
    pub ci_cl: [f64; 2],
    pub h_l: f64,
    pub h_r: f64,
    pub b_l: f64,
    pub b_r: f64,
    pub n_h_l: usize,
    pub n_h_r: usize,
    pub weight: f64,
}

/// Output of `rdmc()`.
#[derive(Clone, Debug)]
pub struct RdMcResult {
    /// Pooled estimate (rdrobust on X - C with cutoff 0).
    pub pooled: CutoffResult,
    /// Weighted average of cutoff-specific estimates.
    pub weighted: WeightedResult,
    /// Per-cutoff results.
    pub cutoffs: Vec<CutoffResult>,
    /// Cutoffs where rdrobust failed.
    pub cfail: Vec<f64>,
}

#[derive(Clone, Debug)]
pub struct WeightedResult {
    pub tau_bc: f64,
    pub se_rb: f64,
    pub pv_rb: f64,
    pub ci_rb: [f64; 2],
    pub tau_cl: f64,
    pub se_cl: f64,
    pub pv_cl: f64,
    pub ci_cl: [f64; 2],
}

/// Configuration for `rdmc()`.
#[derive(Clone, Debug)]
pub struct RdMcConfig {
    pub y: Vec<f64>,
    pub x: Vec<f64>,
    pub c: Vec<f64>, // cutoff variable (per observation)
    pub p: usize,
    pub kernel: Kernel,
    pub bwselect: String,
    pub vce: String,
    pub level: f64,
}

impl Default for RdMcConfig {
    fn default() -> Self {
        Self {
            y: vec![],
            x: vec![],
            c: vec![],
            p: 1,
            kernel: Kernel::Triangular,
            bwselect: "mserd".into(),
            vce: "nn".into(),
            level: 95.0,
        }
    }
}

/// Multi-cutoff RD estimation.
///
/// Faithful port of `rdmulti::rdmc()`. Runs rdrobust on pooled data
/// (X - C with cutoff 0) and per-cutoff subsets, then computes a
/// sample-size-weighted average.
pub fn rdmc(cfg: &RdMcConfig) -> Result<RdMcResult, RdMultiError> {
    let mut clist: Vec<f64> = cfg.c.iter().copied().collect();
    clist.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    clist.dedup();
    let cnum = clist.len();

    // Centered running variable for pooled estimate
    let xc: Vec<f64> = cfg.x.iter().zip(&cfg.c).map(|(xi, ci)| xi - ci).collect();

    // --- Pooled estimate: rdrobust on (Y, X-C) with cutoff 0 ---
    let pooled_rd = rdrobust(&RdRobustConfig {
        y: cfg.y.clone(),
        x: xc.clone(),
        c: 0.0,
        p: cfg.p,
        q: cfg.p + 1,
        kernel: cfg.kernel,
        bwselect: cfg.bwselect.clone(),
        vce: cfg.vce.clone(),
        level: cfg.level,
        ..Default::default()
    })?;

    let pooled = cutoff_from_rd(&pooled_rd, 0.0, 1.0);

    // --- Per-cutoff estimates ---
    let mut cutoffs: Vec<CutoffResult> = Vec::with_capacity(cnum);
    let mut cfail = Vec::new();

    for &cv in &clist {
        // Select observations at this cutoff
        let mask: Vec<bool> = cfg
            .c
            .iter()
            .map(|ci| (ci - cv).abs() <= f64::EPSILON)
            .collect();
        let yc: Vec<f64> = cfg
            .y
            .iter()
            .zip(&mask)
            .filter(|(_, m)| **m)
            .map(|(y, _)| *y)
            .collect();
        let xc_sub: Vec<f64> = xc
            .iter()
            .zip(&mask)
            .filter(|(_, m)| **m)
            .map(|(x, _)| *x)
            .collect();

        if yc.len() < 20 {
            cfail.push(cv);
            continue;
        }

        match rdrobust(&RdRobustConfig {
            y: yc,
            x: xc_sub,
            c: 0.0,
            p: cfg.p,
            q: cfg.p + 1,
            kernel: cfg.kernel,
            bwselect: cfg.bwselect.clone(),
            vce: cfg.vce.clone(),
            level: cfg.level,
            ..Default::default()
        }) {
            Ok(rd) => {
                // Weight will be filled later
                cutoffs.push(cutoff_from_rd(&rd, cv, 0.0));
            }
            Err(_) => {
                cfail.push(cv);
            }
        }
    }

    // --- Compute weights (proportional to effective sample size) ---
    let total_nh: usize = cutoffs.iter().map(|c| c.n_h_l + c.n_h_r).sum();
    for c in &mut cutoffs {
        c.weight = if total_nh > 0 {
            (c.n_h_l + c.n_h_r) as f64 / total_nh as f64
        } else {
            0.0
        };
    }

    // --- Weighted estimate ---
    let normal = Normal::new(0.0, 1.0).unwrap();
    let quant = normal.inverse_cdf(1.0 - (1.0 - cfg.level / 100.0) / 2.0);

    let w_tau_bc: f64 = cutoffs.iter().map(|c| c.tau_bc * c.weight).sum();
    let w_var_rb: f64 = cutoffs
        .iter()
        .map(|c| c.se_rb.powi(2) * c.weight.powi(2))
        .sum();
    let w_se_rb = w_var_rb.sqrt();
    let w_pv_rb = 2.0 * normal.cdf(-(w_tau_bc / w_se_rb).abs());
    let w_ci_rb = [w_tau_bc - quant * w_se_rb, w_tau_bc + quant * w_se_rb];

    let w_tau_cl: f64 = cutoffs.iter().map(|c| c.tau_cl * c.weight).sum();
    let w_var_cl: f64 = cutoffs
        .iter()
        .map(|c| c.se_cl.powi(2) * c.weight.powi(2))
        .sum();
    let w_se_cl = w_var_cl.sqrt();
    let w_pv_cl = 2.0 * normal.cdf(-(w_tau_cl / w_se_cl).abs());
    let w_ci_cl = [w_tau_cl - quant * w_se_cl, w_tau_cl + quant * w_se_cl];

    let weighted = WeightedResult {
        tau_bc: w_tau_bc,
        se_rb: w_se_rb,
        pv_rb: w_pv_rb,
        ci_rb: w_ci_rb,
        tau_cl: w_tau_cl,
        se_cl: w_se_cl,
        pv_cl: w_pv_cl,
        ci_cl: w_ci_cl,
    };

    Ok(RdMcResult {
        pooled,
        weighted,
        cutoffs,
        cfail,
    })
}

fn cutoff_from_rd(rd: &RdRobustOutput, cutoff: f64, weight: f64) -> CutoffResult {
    CutoffResult {
        cutoff,
        tau_cl: rd.tau_cl,
        tau_bc: rd.tau_bc,
        se_cl: rd.se_cl,
        se_rb: rd.se_rb,
        pv_rb: rd.pv[2],
        pv_cl: rd.pv[0],
        ci_rb: rd.ci[2],
        ci_cl: rd.ci[0],
        h_l: rd.h_l,
        h_r: rd.h_r,
        b_l: rd.b_l,
        b_r: rd.b_r,
        n_h_l: rd.n_h_l,
        n_h_r: rd.n_h_r,
        weight,
    }
}

// =====================================================================
// rdms: Multi-score RD
// =====================================================================

/// Configuration for `rdms()`.
#[derive(Clone, Debug)]
pub struct RdMsConfig {
    pub y: Vec<f64>,
    pub x: Vec<f64>,
    pub c: Vec<f64>,             // cutoffs (one per cutoff, not per observation)
    pub x2: Option<Vec<f64>>,    // second running variable
    pub zvar: Option<Vec<f64>>,  // treatment indicator (for X2)
    pub c2: Option<Vec<f64>>,    // second cutoffs
    pub xnorm: Option<Vec<f64>>, // normalized running variable for pooled
    pub p: usize,
    pub kernel: Kernel,
    pub bwselect: String,
    pub vce: String,
    pub level: f64,
}

impl Default for RdMsConfig {
    fn default() -> Self {
        Self {
            y: vec![],
            x: vec![],
            c: vec![],
            x2: None,
            zvar: None,
            c2: None,
            xnorm: None,
            p: 1,
            kernel: Kernel::Triangular,
            bwselect: "mserd".into(),
            vce: "nn".into(),
            level: 95.0,
        }
    }
}

/// Output of `rdms()`.
#[derive(Clone, Debug)]
pub struct RdMsResult {
    /// Per-cutoff results.
    pub cutoffs: Vec<CutoffResult>,
    /// Optional pooled estimate (when xnorm is provided).
    pub pooled: Option<CutoffResult>,
}

/// Multi-score RD estimation.
///
/// Faithful port of `rdmulti::rdms()`. Runs rdrobust per cutoff
/// (optionally using 2D distance), and optionally a pooled estimate.
pub fn rdms(cfg: &RdMsConfig) -> Result<RdMsResult, RdMultiError> {
    let cnum = cfg.c.len();
    let mut cutoffs: Vec<CutoffResult> = Vec::with_capacity(cnum);

    for i in 0..cnum {
        let cv = cfg.c[i];

        let xc_full: Vec<f64> = if let (Some(x2), Some(zvar), Some(c2)) =
            (&cfg.x2, &cfg.zvar, &cfg.c2)
        {
            // 2D: Euclidean distance * sign(zvar)
            let c2v = c2[i];
            (0..cfg.x.len())
                .map(|j| {
                    ((cfg.x[j] - cv).powi(2) + (x2[j] - c2v).powi(2)).sqrt() * (2.0 * zvar[j] - 1.0)
                })
                .collect()
        } else {
            // 1D: X - C[i]
            cfg.x.iter().map(|xi| xi - cv).collect()
        };

        match rdrobust(&RdRobustConfig {
            y: cfg.y.clone(),
            x: xc_full,
            c: 0.0,
            p: cfg.p,
            q: cfg.p + 1,
            kernel: cfg.kernel,
            bwselect: cfg.bwselect.clone(),
            vce: cfg.vce.clone(),
            level: cfg.level,
            ..Default::default()
        }) {
            Ok(rd) => cutoffs.push(cutoff_from_rd(&rd, cv, 0.0)),
            Err(_) => continue,
        }
    }

    // Optional pooled estimate
    let pooled = if let Some(xnorm) = &cfg.xnorm {
        match rdrobust(&RdRobustConfig {
            y: cfg.y.clone(),
            x: xnorm.clone(),
            c: 0.0,
            p: cfg.p,
            q: cfg.p + 1,
            kernel: cfg.kernel,
            bwselect: cfg.bwselect.clone(),
            vce: cfg.vce.clone(),
            level: cfg.level,
            ..Default::default()
        }) {
            Ok(rd) => Some(cutoff_from_rd(&rd, 0.0, 1.0)),
            Err(_) => None,
        }
    } else {
        None
    };

    Ok(RdMsResult { cutoffs, pooled })
}
