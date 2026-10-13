//! `rdrobust`: Rust port of the R `rdrobust` package.
//!
//! Local-polynomial-based inference for regression discontinuity designs
//! with robust bias correction. Faithful port of Calonico, Cattaneo &
//! Titiunik (2014, 2015) and Calonico, Cattaneo, Farrell & Titiunik (2017).

pub mod bw;
pub mod helpers;
pub mod vce;

pub use helpers::Kernel;

use faer::Mat;
use statrs::distribution::{ContinuousCDF, Normal};
use thiserror::Error;

use helpers::*;
use vce::{rdrobust_res, rdrobust_vce};

// =====================================================================
// Error
// =====================================================================

#[derive(Debug, Error)]
pub enum RdRobustError {
    #[error("{0}")]
    Msg(String),
    #[error("not enough observations ({0} < 20)")]
    TooFewObs(usize),
    #[error("invalid input: {0}")]
    InvalidInput(String),
}

// =====================================================================
// Configuration
// =====================================================================

#[derive(Clone, Debug)]
pub struct RdRobustConfig {
    pub y: Vec<f64>,
    pub x: Vec<f64>,
    pub c: f64,
    pub deriv: usize,
    pub p: usize,
    pub q: usize,
    pub h: Option<[Option<f64>; 2]>, // (h_l, h_r), scalar if both same
    pub b: Option<[Option<f64>; 2]>,
    pub rho: Option<f64>,
    pub kernel: Kernel,
    pub bwselect: String,
    pub vce: String, // resolved vce: "nn", "hc0".."hc3", "cr1", "cr2", "cr3"
    pub cluster: Option<Vec<f64>>,
    pub nnmatch: usize,
    pub level: f64,
    pub scalepar: f64,
    pub scaleregul: f64,
    pub sharpbw: bool,
    pub fuzzy: Option<Vec<f64>>,
    pub covs: Option<Mat<f64>>,
    pub covs_drop: bool,
    pub weights: Option<Vec<f64>>,
    pub masspoints: String, // "adjust", "check", "off"
    pub bwcheck: Option<usize>,
    pub bwrestrict: bool,
    pub stdvars: bool,
}

impl Default for RdRobustConfig {
    fn default() -> Self {
        Self {
            y: vec![],
            x: vec![],
            c: 0.0,
            deriv: 0,
            p: 1,
            q: 2,
            h: None,
            b: None,
            rho: None,
            kernel: Kernel::Triangular,
            bwselect: "mserd".into(),
            vce: "nn".into(),
            cluster: None,
            nnmatch: 3,
            level: 95.0,
            scalepar: 1.0,
            scaleregul: 1.0,
            sharpbw: false,
            fuzzy: None,
            covs: None,
            covs_drop: true,
            weights: None,
            masspoints: "adjust".into(),
            bwcheck: None,
            bwrestrict: true,
            stdvars: false,
        }
    }
}

// =====================================================================
// Output
// =====================================================================

#[derive(Clone, Debug)]
pub struct RdRobustOutput {
    /// Conventional point estimate (tau.us).
    pub tau_cl: f64,
    /// Bias-corrected point estimate (tau.bc).
    pub tau_bc: f64,
    /// Conventional standard error.
    pub se_cl: f64,
    /// Robust bias-corrected standard error.
    pub se_rb: f64,
    /// z-statistics: [conventional, bias-corrected, robust].
    pub z: [f64; 3],
    /// p-values: [conventional, bias-corrected, robust].
    pub pv: [f64; 3],
    /// Confidence intervals: [conventional, bias-corrected, robust] × [lower, upper].
    pub ci: [[f64; 2]; 3],
    /// Bandwidths: (left, right) for main (h) and bias (b).
    pub h_l: f64,
    pub h_r: f64,
    pub b_l: f64,
    pub b_r: f64,
    /// Sample sizes.
    pub n_l: usize,
    pub n_r: usize,
    pub n_h_l: usize,
    pub n_h_r: usize,
    pub n_b_l: usize,
    pub n_b_r: usize,
    /// Per-side bias (before normalization by h^(1+p-deriv)).
    pub bias_l: f64,
    pub bias_r: f64,
    /// Per-side variance matrices (full (p+1)×(p+1) or (q+1)×(q+1)).
    pub v_cl_l: Mat<f64>,
    pub v_cl_r: Mat<f64>,
    pub v_rb_l: Mat<f64>,
    pub v_rb_r: Mat<f64>,
    /// Metadata.
    pub kernel: String,
    pub vce_type: String,
    pub bwselect: String,
    pub p: usize,
    pub q: usize,
    pub deriv: usize,
    pub level: f64,
    /// Per-side conventional estimates.
    pub tau_cl_l: f64,
    pub tau_cl_r: f64,
    pub tau_bc_l: f64,
    pub tau_bc_r: f64,
    /// Polynomial coefficient vectors (per side).
    pub beta_y_p_l: Vec<f64>,
    pub beta_y_p_r: Vec<f64>,
}

// =====================================================================
// Main rdrobust function
// =====================================================================

/// Perform local-polynomial RD estimation with robust bias correction.
///
/// Faithful port of the R `rdrobust::rdrobust()` function.
pub fn rdrobust(cfg: &RdRobustConfig) -> Result<RdRobustOutput, RdRobustError> {
    let p = cfg.p;
    let q = if cfg.q > p { cfg.q } else { p + 1 };
    let deriv = cfg.deriv.min(p);
    let kernel = cfg.kernel;
    let scalepar = cfg.scalepar;
    let scaleregul = cfg.scaleregul;

    // ----- 1. NA removal -----
    let n_orig = cfg.x.len();
    let na_ok: Vec<bool> = (0..n_orig)
        .map(|i| {
            cfg.x[i].is_finite()
                && cfg.y[i].is_finite()
                && cfg.cluster.as_ref().is_none_or(|c| c[i].is_finite())
                && cfg.fuzzy.as_ref().is_none_or(|f| f[i].is_finite())
                && cfg
                    .weights
                    .as_ref()
                    .is_none_or(|w| w[i].is_finite() && w[i] >= 0.0)
                && cfg
                    .covs
                    .as_ref()
                    .is_none_or(|cv| (0..cv.ncols()).all(|j| cv[(i, j)].is_finite()))
        })
        .collect();

    let mut x: Vec<f64> = (0..n_orig)
        .filter(|&i| na_ok[i])
        .map(|i| cfg.x[i])
        .collect();
    let mut y: Vec<f64> = (0..n_orig)
        .filter(|&i| na_ok[i])
        .map(|i| cfg.y[i])
        .collect();
    let cluster: Option<Vec<f64>> = cfg
        .cluster
        .as_ref()
        .map(|c| (0..n_orig).filter(|&i| na_ok[i]).map(|i| c[i]).collect());
    let fuzzy: Option<Vec<f64>> = cfg
        .fuzzy
        .as_ref()
        .map(|f| (0..n_orig).filter(|&i| na_ok[i]).map(|i| f[i]).collect());
    let weights: Option<Vec<f64>> = cfg
        .weights
        .as_ref()
        .map(|w| (0..n_orig).filter(|&i| na_ok[i]).map(|i| w[i]).collect());
    let covs: Option<Mat<f64>> = cfg.covs.as_ref().map(|cv| {
        let kept: Vec<usize> = (0..n_orig).filter(|&i| na_ok[i]).collect();
        Mat::from_fn(kept.len(), cv.ncols(), |i, j| cv[(kept[i], j)])
    });

    let n = x.len();
    if n < 20 {
        return Err(RdRobustError::TooFewObs(n));
    }

    // ----- 2. Sort by x -----
    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by(|&a, &b| x[a].partial_cmp(&x[b]).unwrap_or(std::cmp::Ordering::Equal));
    x = order.iter().map(|&i| x[i]).collect();
    y = order.iter().map(|&i| y[i]).collect();
    let cluster = cluster.map(|c| order.iter().map(|&i| c[i]).collect::<Vec<_>>());
    let _fuzzy = fuzzy.map(|f| order.iter().map(|&i| f[i]).collect::<Vec<_>>());
    let weights = weights.map(|w| order.iter().map(|&i| w[i]).collect::<Vec<_>>());
    let _covs = covs.map(|cv| Mat::from_fn(cv.nrows(), cv.ncols(), |i, j| cv[(order[i], j)]));

    let c = cfg.c;

    // ----- 3. Standardization -----
    let mut x_sd = 1.0;
    let mut y_sd = 1.0;
    if cfg.h.is_none() && cfg.stdvars {
        y_sd = sd(&y);
        x_sd = sd(&x);
        for v in &mut y {
            *v /= y_sd;
        }
        for v in &mut x {
            *v /= x_sd;
        }
    }
    let c_std = if cfg.h.is_none() && cfg.stdvars {
        c / x_sd
    } else {
        c
    };

    // ----- 4. Split at cutoff -----
    let x_l: Vec<f64> = x.iter().filter(|v| **v < c_std).copied().collect();
    let y_l: Vec<f64> = y
        .iter()
        .zip(&x)
        .filter(|(_, xi)| **xi < c_std)
        .map(|(yi, _)| *yi)
        .collect();
    let x_r: Vec<f64> = x.iter().filter(|v| **v >= c_std).copied().collect();
    let y_r: Vec<f64> = y
        .iter()
        .zip(&x)
        .filter(|(_, xi)| **xi >= c_std)
        .map(|(yi, _)| *yi)
        .collect();

    let n_l = x_l.len();
    let n_r = x_r.len();
    if n_l < p + 2 || n_r < p + 2 {
        return Err(RdRobustError::TooFewObs(n_l + n_r));
    }

    let x_min = *x.first().unwrap();
    let x_max = *x.last().unwrap();
    let range_l = (c_std - x_min).abs();
    let range_r = (x_max - c_std).abs();

    let level = cfg.level;
    let quant = Normal::new(0.0, 1.0)
        .unwrap()
        .inverse_cdf(1.0 - (1.0 - level / 100.0) / 2.0);

    // ----- 5. Resolve VCE -----
    let vce_user = cfg.vce.to_lowercase();
    let cluster_present = cluster.is_some();

    let (vce_internal, vce_type, crv3, crv2) = if cluster_present {
        match vce_user.as_str() {
            "cr1" | "nn" | "hc0" | "" => ("hc1", "CR1", false, false),
            "cr2" | "hc2" => ("crv2", "CR2", false, true),
            "cr3" | "hc3" => ("crv3", "CR3", true, false),
            _ => ("hc1", "CR1", false, false),
        }
    } else {
        match vce_user.as_str() {
            "nn" | "" => ("nn", "NN", false, false),
            "hc0" => ("hc0", "HC0", false, false),
            "hc1" | "cr1" => ("hc1", "HC1", false, false),
            "hc2" | "cr2" => ("hc2", "HC2", false, false),
            "hc3" | "cr3" => ("hc3", "HC3", false, false),
            _ => ("nn", "NN", false, false),
        }
    };
    let vce = vce_internal;

    // Cluster split
    let (c_l, c_r): (Option<Vec<f64>>, Option<Vec<f64>>) = if let Some(cl) = &cluster {
        let cl_l: Vec<f64> = x
            .iter()
            .zip(cl)
            .filter(|(xi, _)| **xi < c_std)
            .map(|(_, &ci)| ci)
            .collect();
        let cl_r: Vec<f64> = x
            .iter()
            .zip(cl)
            .filter(|(xi, _)| **xi >= c_std)
            .map(|(_, &ci)| ci)
            .collect();
        (Some(cl_l), Some(cl_r))
    } else {
        (None, None)
    };

    // Weights split
    let (_fw_l, _fw_r): (Option<Vec<f64>>, Option<Vec<f64>>) = if let Some(w) = &weights {
        let wl: Vec<f64> = x
            .iter()
            .zip(w)
            .filter(|(xi, _)| **xi < c_std)
            .map(|(_, &wi)| wi)
            .collect();
        let wr: Vec<f64> = x
            .iter()
            .zip(w)
            .filter(|(xi, _)| **xi >= c_std)
            .map(|(_, &wi)| wi)
            .collect();
        (Some(wl), Some(wr))
    } else {
        (None, None)
    };

    // ----- 6. Dups/dupsid for NN -----
    let (dups_l, dupsid_l) = dups_dupsid(&x_l);
    let (dups_r, dupsid_r) = dups_dupsid(&x_r);

    // ----- 7. Masspoints -----
    let masspoints = cfg.masspoints.as_str();
    let mut bwcheck = cfg.bwcheck;

    if masspoints == "check" || masspoints == "adjust" {
        let x_uniq_l: Vec<f64> = {
            let mut v = x_l.clone();
            v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            v.dedup();
            v
        };
        let x_uniq_r: Vec<f64> = {
            let mut v = x_r.clone();
            v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            v.dedup();
            v
        };
        let m_l = x_uniq_l.len();
        let m_r = x_uniq_r.len();
        let mass_l = 1.0 - m_l as f64 / n_l as f64;
        let mass_r = 1.0 - m_r as f64 / n_r as f64;
        if (mass_l >= 0.2 || mass_r >= 0.2) && masspoints == "adjust" && bwcheck.is_none() {
            bwcheck = Some(10);
        }
    }

    // ----- 8. Bandwidth selection -----
    let bwselect_lower = cfg.bwselect.to_lowercase();

    let (h_l, h_r, b_l, b_r) = if let Some(h_spec) = cfg.h {
        // Manual bandwidth
        let h_l_val = h_spec[0].or(h_spec[1]).unwrap_or(range_l);
        let h_r_val = h_spec[1].or(h_spec[0]).unwrap_or(range_r);

        let (b_l_val, b_r_val) = if let Some(b_spec) = cfg.b {
            let bl = b_spec[0].or(b_spec[1]).unwrap_or(h_l_val);
            let br = b_spec[1].or(b_spec[0]).unwrap_or(h_r_val);
            (bl, br)
        } else if let Some(rho) = cfg.rho {
            (h_l_val / rho, h_r_val / rho)
        } else {
            (h_l_val, h_r_val)
        };
        (h_l_val, h_r_val, b_l_val, b_r_val)
    } else {
        // Bandwidth selection
        let bw_max_l = (c_std - x_min).abs();
        let bw_max_r = (x_max - c_std).abs();
        let bw_max = bw_max_l.max(bw_max_r);

        // Preliminary bandwidth c_bw
        let bw_p = sd(&x).min(iqr(&x) / 1.349);
        let c_c = match kernel {
            Kernel::Epanechnikov => 2.34,
            Kernel::Uniform => 1.843,
            Kernel::Triangular => 2.576,
        };

        let mut m_total = n_l + n_r;
        if masspoints == "check" || masspoints == "adjust" {
            let x_uniq_l: Vec<f64> = {
                let mut v = x_l.clone();
                v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
                v.dedup();
                v
            };
            let x_uniq_r: Vec<f64> = {
                let mut v = x_r.clone();
                v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
                v.dedup();
                v
            };
            m_total = x_uniq_l.len() + x_uniq_r.len();
        }

        let mut c_bw = c_c * bw_p * (n as f64).powf(-1.0 / 5.0);
        if masspoints == "adjust" {
            c_bw = c_c * bw_p * (m_total as f64).powf(-1.0 / 5.0);
        }
        if cfg.bwrestrict {
            c_bw = c_bw.min(bw_max);
        }

        // bwcheck adjustment
        let (_bw_min_l, _bw_min_r) = if let Some(bc) = bwcheck {
            let x_uniq_l_sorted: Vec<f64> = {
                let mut v = x_l.clone();
                v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
                v.dedup();
                v
            };
            let x_uniq_r_sorted: Vec<f64> = {
                let mut v = x_r.clone();
                v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
                v.dedup();
                v
            };
            let bc_l = bc.min(x_uniq_l_sorted.len());
            let bc_r = bc.min(x_uniq_r_sorted.len());
            let dist_l: Vec<f64> = x_uniq_l_sorted.iter().map(|v| (v - c_std).abs()).collect();
            let dist_r: Vec<f64> = x_uniq_r_sorted.iter().map(|v| (v - c_std).abs()).collect();
            let bml = dist_l.get(bc_l - 1).copied().unwrap_or(0.0);
            let bmr = dist_r.get(bc_r - 1).copied().unwrap_or(0.0);
            c_bw = c_bw.max(bml).max(bmr);
            (bml, bmr)
        } else {
            (0.0, 0.0)
        };

        // Pilot bandwidth calculations
        // Step 1: d_bw (preliminary)
        let c_d_l = bw::rdrobust_bw(
            &y_l,
            &x_l,
            c_std,
            q + 1,
            q + 1,
            q + 2,
            c_bw,
            range_l,
            0.0,
            vce,
            cfg.nnmatch,
            kernel,
            &dups_l,
            &dupsid_l,
            c_l.as_deref(),
        );
        let c_d_r = bw::rdrobust_bw(
            &y_r,
            &x_r,
            c_std,
            q + 1,
            q + 1,
            q + 2,
            c_bw,
            range_r,
            0.0,
            vce,
            cfg.nnmatch,
            kernel,
            &dups_r,
            &dupsid_r,
            c_r.as_deref(),
        );

        // MSE-RD (default)
        let need_rd = matches!(
            bwselect_lower.as_str(),
            "mserd" | "cerrd" | "msecomb1" | "msecomb2" | "cercomb1" | "cercomb2" | ""
        );
        let need_two = matches!(
            bwselect_lower.as_str(),
            "msetwo" | "certwo" | "msecomb2" | "cercomb2"
        );
        let need_sum = matches!(
            bwselect_lower.as_str(),
            "msesum" | "cersum" | "msecomb1" | "msecomb2" | "cercomb1" | "cercomb2"
        );

        let mut h_bw_d = 0.0;
        let mut b_bw_d = 0.0;
        let mut h_bw_s = 0.0;
        let mut b_bw_s = 0.0;
        let mut h_bw_l = 0.0;
        let mut h_bw_r_val = 0.0;
        let mut b_bw_l = 0.0;
        let mut b_bw_r_val = 0.0;

        if need_two {
            let d_bw_l = (c_d_l.v / c_d_l.b.powi(2)).powf(c_d_l.rate);
            let d_bw_r = (c_d_r.v / c_d_r.b.powi(2)).powf(c_d_r.rate);
            let d_bw_l = if cfg.bwrestrict {
                d_bw_l.min(bw_max_l)
            } else {
                d_bw_l
            };
            let d_bw_r = if cfg.bwrestrict {
                d_bw_r.min(bw_max_r)
            } else {
                d_bw_r
            };

            let c_b_l = bw::rdrobust_bw(
                &y_l,
                &x_l,
                c_std,
                q,
                p + 1,
                q + 1,
                c_bw,
                d_bw_l,
                scaleregul,
                vce,
                cfg.nnmatch,
                kernel,
                &dups_l,
                &dupsid_l,
                c_l.as_deref(),
            );
            let c_b_r = bw::rdrobust_bw(
                &y_r,
                &x_r,
                c_std,
                q,
                p + 1,
                q + 1,
                c_bw,
                d_bw_r,
                scaleregul,
                vce,
                cfg.nnmatch,
                kernel,
                &dups_r,
                &dupsid_r,
                c_r.as_deref(),
            );
            b_bw_l = (c_b_l.v / (c_b_l.b.powi(2) + scaleregul * c_b_l.r)).powf(c_b_l.rate);
            b_bw_r_val = (c_b_r.v / (c_b_r.b.powi(2) + scaleregul * c_b_r.r)).powf(c_b_r.rate);
            let b_bw_l = if cfg.bwrestrict {
                b_bw_l.min(bw_max_l)
            } else {
                b_bw_l
            };
            let b_bw_r_val = if cfg.bwrestrict {
                b_bw_r_val.min(bw_max_r)
            } else {
                b_bw_r_val
            };

            let c_h_l = bw::rdrobust_bw(
                &y_l,
                &x_l,
                c_std,
                p,
                deriv,
                q,
                c_bw,
                b_bw_l,
                scaleregul,
                vce,
                cfg.nnmatch,
                kernel,
                &dups_l,
                &dupsid_l,
                c_l.as_deref(),
            );
            let c_h_r = bw::rdrobust_bw(
                &y_r,
                &x_r,
                c_std,
                p,
                deriv,
                q,
                c_bw,
                b_bw_r_val,
                scaleregul,
                vce,
                cfg.nnmatch,
                kernel,
                &dups_r,
                &dupsid_r,
                c_r.as_deref(),
            );
            h_bw_l = (c_h_l.v / (c_h_l.b.powi(2) + scaleregul * c_h_l.r)).powf(c_h_l.rate);
            h_bw_r_val = (c_h_r.v / (c_h_r.b.powi(2) + scaleregul * c_h_r.r)).powf(c_h_r.rate);
            let _h_bw_l = if cfg.bwrestrict {
                h_bw_l.min(bw_max_l)
            } else {
                h_bw_l
            };
            let _h_bw_r_val = if cfg.bwrestrict {
                h_bw_r_val.min(bw_max_r)
            } else {
                h_bw_r_val
            };
        }

        if need_sum {
            let d_bw_s = ((c_d_l.v + c_d_r.v) / (c_d_r.b + c_d_l.b).powi(2)).powf(c_d_l.rate);
            let d_bw_s = if cfg.bwrestrict {
                d_bw_s.min(bw_max)
            } else {
                d_bw_s
            };

            let c_b_l = bw::rdrobust_bw(
                &y_l,
                &x_l,
                c_std,
                q,
                p + 1,
                q + 1,
                c_bw,
                d_bw_s,
                scaleregul,
                vce,
                cfg.nnmatch,
                kernel,
                &dups_l,
                &dupsid_l,
                c_l.as_deref(),
            );
            let c_b_r = bw::rdrobust_bw(
                &y_r,
                &x_r,
                c_std,
                q,
                p + 1,
                q + 1,
                c_bw,
                d_bw_s,
                scaleregul,
                vce,
                cfg.nnmatch,
                kernel,
                &dups_r,
                &dupsid_r,
                c_r.as_deref(),
            );
            b_bw_s = ((c_b_l.v + c_b_r.v)
                / ((c_b_r.b + c_b_l.b).powi(2) + scaleregul * (c_b_r.r + c_b_l.r)))
                .powf(c_b_l.rate);
            let b_bw_s = if cfg.bwrestrict {
                b_bw_s.min(bw_max)
            } else {
                b_bw_s
            };

            let c_h_l = bw::rdrobust_bw(
                &y_l,
                &x_l,
                c_std,
                p,
                deriv,
                q,
                c_bw,
                b_bw_s,
                scaleregul,
                vce,
                cfg.nnmatch,
                kernel,
                &dups_l,
                &dupsid_l,
                c_l.as_deref(),
            );
            let c_h_r = bw::rdrobust_bw(
                &y_r,
                &x_r,
                c_std,
                p,
                deriv,
                q,
                c_bw,
                b_bw_s,
                scaleregul,
                vce,
                cfg.nnmatch,
                kernel,
                &dups_r,
                &dupsid_r,
                c_r.as_deref(),
            );
            h_bw_s = ((c_h_l.v + c_h_r.v)
                / ((c_h_r.b + c_h_l.b).powi(2) + scaleregul * (c_h_r.r + c_h_l.r)))
                .powf(c_h_l.rate);
            let _h_bw_s = if cfg.bwrestrict {
                h_bw_s.min(bw_max)
            } else {
                h_bw_s
            };
        }

        if need_rd {
            let d_bw_d = ((c_d_l.v + c_d_r.v) / (c_d_r.b - c_d_l.b).powi(2)).powf(c_d_l.rate);
            let d_bw_d = if cfg.bwrestrict {
                d_bw_d.min(bw_max)
            } else {
                d_bw_d
            };

            let c_b_l = bw::rdrobust_bw(
                &y_l,
                &x_l,
                c_std,
                q,
                p + 1,
                q + 1,
                c_bw,
                d_bw_d,
                scaleregul,
                vce,
                cfg.nnmatch,
                kernel,
                &dups_l,
                &dupsid_l,
                c_l.as_deref(),
            );
            let c_b_r = bw::rdrobust_bw(
                &y_r,
                &x_r,
                c_std,
                q,
                p + 1,
                q + 1,
                c_bw,
                d_bw_d,
                scaleregul,
                vce,
                cfg.nnmatch,
                kernel,
                &dups_r,
                &dupsid_r,
                c_r.as_deref(),
            );
            b_bw_d = ((c_b_l.v + c_b_r.v)
                / ((c_b_r.b - c_b_l.b).powi(2) + scaleregul * (c_b_r.r + c_b_l.r)))
                .powf(c_b_l.rate);
            let b_bw_d = if cfg.bwrestrict {
                b_bw_d.min(bw_max)
            } else {
                b_bw_d
            };

            let c_h_l = bw::rdrobust_bw(
                &y_l,
                &x_l,
                c_std,
                p,
                deriv,
                q,
                c_bw,
                b_bw_d,
                scaleregul,
                vce,
                cfg.nnmatch,
                kernel,
                &dups_l,
                &dupsid_l,
                c_l.as_deref(),
            );
            let c_h_r = bw::rdrobust_bw(
                &y_r,
                &x_r,
                c_std,
                p,
                deriv,
                q,
                c_bw,
                b_bw_d,
                scaleregul,
                vce,
                cfg.nnmatch,
                kernel,
                &dups_r,
                &dupsid_r,
                c_r.as_deref(),
            );
            h_bw_d = ((c_h_l.v + c_h_r.v)
                / ((c_h_r.b - c_h_l.b).powi(2) + scaleregul * (c_h_r.r + c_h_l.r)))
                .powf(c_h_l.rate);
            let _h_bw_d = if cfg.bwrestrict {
                h_bw_d.min(bw_max)
            } else {
                h_bw_d
            };
        }

        // Select final bandwidths based on bwselect
        let (h_bw_final_l, h_bw_final_r, b_bw_final_l, b_bw_final_r) = match bwselect_lower.as_str()
        {
            "mserd" | "" => {
                let h = x_sd * h_bw_d;
                let b = x_sd * b_bw_d;
                (h, h, b, b)
            }
            "msesum" => {
                let h = x_sd * h_bw_s;
                let b = x_sd * b_bw_s;
                (h, h, b, b)
            }
            "msetwo" => (
                x_sd * h_bw_l,
                x_sd * h_bw_r_val,
                x_sd * b_bw_l,
                x_sd * b_bw_r_val,
            ),
            "msecomb1" => {
                let h_rd = x_sd * h_bw_d;
                let h_sum = x_sd * h_bw_s;
                let b_rd = x_sd * b_bw_d;
                let b_sum = x_sd * b_bw_s;
                let h = h_rd.min(h_sum);
                let b = b_rd.min(b_sum);
                (h, h, b, b)
            }
            "msecomb2" => {
                let h_rd = x_sd * h_bw_d;
                let h_sum = x_sd * h_bw_s;
                let h_two_l = x_sd * h_bw_l;
                let h_two_r = x_sd * h_bw_r_val;
                let b_rd = x_sd * b_bw_d;
                let b_sum = x_sd * b_bw_s;
                let b_two_l = x_sd * b_bw_l;
                let b_two_r = x_sd * b_bw_r_val;
                let hl = median(&[h_rd, h_sum, h_two_l]);
                let hr = median(&[h_rd, h_sum, h_two_r]);
                let bl = median(&[b_rd, b_sum, b_two_l]);
                let br = median(&[b_rd, b_sum, b_two_r]);
                (hl, hr, bl, br)
            }
            "cerrd" => {
                let cer_h =
                    (n as f64).powf(-(p as f64 / ((3.0 + p as f64) * (3.0 + 2.0 * p as f64))));
                let h = x_sd * h_bw_d * cer_h;
                let b = x_sd * b_bw_d;
                (h, h, b, b)
            }
            "cersum" => {
                let cer_h =
                    (n as f64).powf(-(p as f64 / ((3.0 + p as f64) * (3.0 + 2.0 * p as f64))));
                let h = x_sd * h_bw_s * cer_h;
                let b = x_sd * b_bw_s;
                (h, h, b, b)
            }
            "certwo" => {
                let cer_h =
                    (n as f64).powf(-(p as f64 / ((3.0 + p as f64) * (3.0 + 2.0 * p as f64))));
                (
                    x_sd * h_bw_l * cer_h,
                    x_sd * h_bw_r_val * cer_h,
                    x_sd * b_bw_l,
                    x_sd * b_bw_r_val,
                )
            }
            "cercomb1" => {
                let cer_h =
                    (n as f64).powf(-(p as f64 / ((3.0 + p as f64) * (3.0 + 2.0 * p as f64))));
                let h_rd = x_sd * h_bw_d;
                let h_sum = x_sd * h_bw_s;
                let b_rd = x_sd * b_bw_d;
                let b_sum = x_sd * b_bw_s;
                let h = h_rd.min(h_sum) * cer_h;
                let b = b_rd.min(b_sum);
                (h, h, b, b)
            }
            "cercomb2" => {
                let cer_h =
                    (n as f64).powf(-(p as f64 / ((3.0 + p as f64) * (3.0 + 2.0 * p as f64))));
                let h_rd = x_sd * h_bw_d;
                let h_sum = x_sd * h_bw_s;
                let h_two_l = x_sd * h_bw_l;
                let h_two_r = x_sd * h_bw_r_val;
                let b_rd = x_sd * b_bw_d;
                let b_sum = x_sd * b_bw_s;
                let b_two_l = x_sd * b_bw_l;
                let b_two_r = x_sd * b_bw_r_val;
                let hl = median(&[h_rd, h_sum, h_two_l]) * cer_h;
                let hr = median(&[h_rd, h_sum, h_two_r]) * cer_h;
                let bl = median(&[b_rd, b_sum, b_two_l]);
                let br = median(&[b_rd, b_sum, b_two_r]);
                (hl, hr, bl, br)
            }
            _ => {
                let h = x_sd * h_bw_d;
                let b = x_sd * b_bw_d;
                (h, h, b, b)
            }
        };

        let h_l = h_bw_final_l;
        let h_r = h_bw_final_r;
        let mut b_l = b_bw_final_l;
        let mut b_r = b_bw_final_r;

        if let Some(rho) = cfg.rho {
            b_l = h_l / rho;
            b_r = h_r / rho;
        }

        (h_l, h_r, b_l, b_r)
    };

    // ----- 9. De-standardize -----
    let c_final = if cfg.h.is_none() && cfg.stdvars {
        c_std * x_sd
    } else {
        c
    };
    let x_l_final: Vec<f64> = if cfg.h.is_none() && cfg.stdvars {
        x_l.iter().map(|v| v * x_sd).collect()
    } else {
        x_l.clone()
    };
    let x_r_final: Vec<f64> = if cfg.h.is_none() && cfg.stdvars {
        x_r.iter().map(|v| v * x_sd).collect()
    } else {
        x_r.clone()
    };
    let y_l_final: Vec<f64> = if cfg.h.is_none() && cfg.stdvars {
        y_l.iter().map(|v| v * y_sd).collect()
    } else {
        y_l.clone()
    };
    let y_r_final: Vec<f64> = if cfg.h.is_none() && cfg.stdvars {
        y_r.iter().map(|v| v * y_sd).collect()
    } else {
        y_r.clone()
    };

    // ----- 10. Estimation -----
    // Kernel weights at h and b
    let w_h_l = kernel_weight(&x_l_final, c_final, h_l, kernel);
    let w_h_r = kernel_weight(&x_r_final, c_final, h_r, kernel);
    let w_b_l = kernel_weight(&x_l_final, c_final, b_l, kernel);
    let w_b_r = kernel_weight(&x_r_final, c_final, b_r, kernel);

    // Effective sample: union of h and b windows
    let ind_l: Vec<bool> = (0..x_l_final.len())
        .map(|i| {
            if h_l > b_l {
                w_h_l[i] > 0.0
            } else {
                w_b_l[i] > 0.0
            }
        })
        .collect();
    let ind_r: Vec<bool> = (0..x_r_final.len())
        .map(|i| {
            if h_r > b_r {
                w_h_r[i] > 0.0
            } else {
                w_b_r[i] > 0.0
            }
        })
        .collect();

    let e_idx_l: Vec<usize> = (0..x_l_final.len()).filter(|&i| ind_l[i]).collect();
    let e_idx_r: Vec<usize> = (0..x_r_final.len()).filter(|&i| ind_r[i]).collect();

    let e_x_l: Vec<f64> = e_idx_l.iter().map(|&i| x_l_final[i]).collect();
    let e_x_r: Vec<f64> = e_idx_r.iter().map(|&i| x_r_final[i]).collect();
    let e_y_l: Vec<f64> = e_idx_l.iter().map(|&i| y_l_final[i]).collect();
    let e_y_r: Vec<f64> = e_idx_r.iter().map(|&i| y_r_final[i]).collect();

    let w_h_l_eff: Vec<f64> = e_idx_l.iter().map(|&i| w_h_l[i]).collect();
    let w_h_r_eff: Vec<f64> = e_idx_r.iter().map(|&i| w_h_r[i]).collect();
    let w_b_l_eff: Vec<f64> = e_idx_l.iter().map(|&i| w_b_l[i]).collect();
    let w_b_r_eff: Vec<f64> = e_idx_r.iter().map(|&i| w_b_r[i]).collect();

    let n_h_l = w_h_l.iter().filter(|&&w| w > 0.0).count();
    let n_h_r = w_h_r.iter().filter(|&&w| w > 0.0).count();
    let n_b_l = w_b_l.iter().filter(|&&w| w > 0.0).count();
    let n_b_r = w_b_r.iter().filter(|&&w| w > 0.0).count();

    let en_l = e_idx_l.len();
    let en_r = e_idx_r.len();

    // Effective dups/dupsid for NN
    let edups_l: Vec<usize> = if vce == "nn" {
        e_idx_l.iter().map(|&i| dups_l[i]).collect()
    } else {
        vec![0; en_l]
    };
    let edupsid_l: Vec<usize> = if vce == "nn" {
        e_idx_l.iter().map(|&i| dupsid_l[i]).collect()
    } else {
        vec![0; en_l]
    };
    let edups_r: Vec<usize> = if vce == "nn" {
        e_idx_r.iter().map(|&i| dups_r[i]).collect()
    } else {
        vec![0; en_r]
    };
    let edupsid_r: Vec<usize> = if vce == "nn" {
        e_idx_r.iter().map(|&i| dupsid_r[i]).collect()
    } else {
        vec![0; en_r]
    };

    // Vandermonde matrices
    let u_l: Vec<f64> = e_x_l.iter().map(|&v| (v - c_final) / h_l).collect();
    let u_r: Vec<f64> = e_x_r.iter().map(|&v| (v - c_final) / h_r).collect();
    let e_xmc_l: Vec<f64> = e_x_l.iter().map(|&v| v - c_final).collect();
    let e_xmc_r: Vec<f64> = e_x_r.iter().map(|&v| v - c_final).collect();

    let r_q_l = vandermonde(&e_xmc_l, q);
    let r_q_r = vandermonde(&e_xmc_r, q);
    // R_p = first (p+1) columns of R_q
    // R_p = first (p+1) columns of R_q
    let r_p_l = Mat::from_fn(en_l, p + 1, |i, j| r_q_l[(i, j)]);
    let r_p_r = Mat::from_fn(en_r, p + 1, |i, j| r_q_r[(i, j)]);

    // invG_p, invG_q
    let sqrt_wh_l: Vec<f64> = w_h_l_eff.iter().map(|w| w.sqrt()).collect();
    let sqrt_wh_r: Vec<f64> = w_h_r_eff.iter().map(|w| w.sqrt()).collect();
    let sqrt_wb_l: Vec<f64> = w_b_l_eff.iter().map(|w| w.sqrt()).collect();
    let sqrt_wb_r: Vec<f64> = w_b_r_eff.iter().map(|w| w.sqrt()).collect();

    let r_p_l_sqrt = scale_rows(&r_p_l, &sqrt_wh_l);
    let r_p_r_sqrt = scale_rows(&r_p_r, &sqrt_wh_r);
    let r_q_l_sqrt = scale_rows(&r_q_l, &sqrt_wb_l);
    let r_q_r_sqrt = scale_rows(&r_q_r, &sqrt_wb_r);

    let inv_g_p_l = qr_xx_inv(&r_p_l_sqrt);
    let inv_g_p_r = qr_xx_inv(&r_p_r_sqrt);
    let inv_g_q_l = qr_xx_inv(&r_q_l_sqrt);
    let inv_g_q_r = qr_xx_inv(&r_q_r_sqrt);

    // L_l, L_r: crossprod(R_p * W_h, u^(p+1))
    let u_l_pp1: Vec<f64> = u_l.iter().map(|v| v.powi((p + 1) as i32)).collect();
    let u_r_pp1: Vec<f64> = u_r.iter().map(|v| v.powi((p + 1) as i32)).collect();

    let l_l = weighted_crossprod(
        &r_p_l,
        &w_h_l_eff,
        &Mat::from_fn(en_l, 1, |i, _| u_l_pp1[i]),
    );
    let l_r = weighted_crossprod(
        &r_p_r,
        &w_h_r_eff,
        &Mat::from_fn(en_r, 1, |i, _| u_r_pp1[i]),
    );

    // e_p1: zero vector of length q+1, with 1 at position p+1 (0-based)
    // Q_q = R_p*W_h - h^(p+1) * (L %*% t(e_p1)) %*% (t(invG_q %*% t(R_q)) * W_b)
    // Q_q[i,j] = R_p[i,j]*W_h[i] - h^(p+1) * L[j] * sum_k (invG_q[k,j]*R_q[i,k]) * W_b[i]

    // Compute t(invG_q %*% t(R_q)) = R_q %*% invG_q' = R_q %*% t(invG_q)
    // Actually: t(t(invG_q %*% t(R_q)) * W_b) = (R_q %*% invG_q')' * diag(W_b) transposed
    // Let's compute: for each column j of Q_q:
    // Q_q[i,j] = R_p[i,j]*W_h[i] - h^(p+1) * L[j] * (sum_k R_q[i,k] * invG_q[k, p+1]) * W_b[i]

    // Because e_p1 selects the (p+1)-th column of invG_q %*% t(R_q)
    // t(invG_q %*% t(R_q)) has dimensions n × (q+1)
    // Actually, invG_q is (q+1)×(q+1), t(R_q) is (q+1)×n, so invG_q %*% t(R_q) is (q+1)×n
    // t(invG_q %*% t(R_q)) is n × (q+1)
    // Multiplying by W_b (element-wise by row) gives n × (q+1)
    // Then t(e_p1) selects column p+1 (0-based: column p+1)

    // The full R formula:
    // Q_q = t(t(R_p*W_h) - h^(p+1)*(L%*%t(e_p1))%*%t(t(invG_q%*%t(R_q))*W_b))
    // Let me simplify: t(R_p*W_h) is (p+1)×n, the subtraction is element-wise.
    // So Q_q = (p+1)×n transposed → n×(p+1)

    // Q_q[i,j] = R_p[i,j]*W_h[i] - h_l^(p+1) * L[j] * sum_k(R_q[i,k]*invG_q_l[k,p+1]) * W_b[i]

    let q_q_l = build_qq(
        &r_p_l, &w_h_l_eff, &r_q_l, &w_b_l_eff, &inv_g_q_l, &l_l, h_l, p, q,
    );
    let q_q_r = build_qq(
        &r_p_r, &w_h_r_eff, &r_q_r, &w_b_r_eff, &inv_g_q_r, &l_r, h_r, p, q,
    );

    // D matrices (just Y for sharp RD)
    let d_l = Mat::from_fn(en_l, 1, |i, _| e_y_l[i]);
    let d_r = Mat::from_fn(en_r, 1, |i, _| e_y_r[i]);

    // Regression coefficients
    let beta_p_l_mat = &inv_g_p_l * &weighted_crossprod(&r_p_l, &w_h_l_eff, &d_l);
    let beta_p_r_mat = &inv_g_p_r * &weighted_crossprod(&r_p_r, &w_h_r_eff, &d_r);
    let beta_q_l_mat = &inv_g_q_l * &weighted_crossprod(&r_q_l, &w_b_l_eff, &d_l);
    let beta_q_r_mat = &inv_g_q_r * &weighted_crossprod(&r_q_r, &w_b_r_eff, &d_r);
    // Q_q already incorporates the kernel weights — use PLAIN crossprod, not weighted.
    let beta_bc_l_mat = &inv_g_p_l * &crossprod(&q_q_l, &d_l);
    let beta_bc_r_mat = &inv_g_p_r * &crossprod(&q_q_r, &d_r);

    // beta_p = beta_p_r - beta_p_l (difference at cutoff)
    // beta_bc = beta_bc_r - beta_bc_l
    let fact_deriv = factorial(deriv);

    // tau_cl = scalepar * factorial(deriv) * (beta_p_r - beta_p_l)[deriv]
    let tau_cl = scalepar * fact_deriv * (beta_p_r_mat[(deriv, 0)] - beta_p_l_mat[(deriv, 0)]);
    let tau_bc = scalepar * fact_deriv * (beta_bc_r_mat[(deriv, 0)] - beta_bc_l_mat[(deriv, 0)]);

    // Per-side estimates
    let tau_y_cl_l = scalepar * fact_deriv * beta_p_l_mat[(deriv, 0)];
    let tau_y_cl_r = scalepar * fact_deriv * beta_p_r_mat[(deriv, 0)];
    let tau_y_bc_l = scalepar * fact_deriv * beta_bc_l_mat[(deriv, 0)];
    let tau_y_bc_r = scalepar * fact_deriv * beta_bc_r_mat[(deriv, 0)];

    let bias_l = tau_y_cl_l - tau_y_bc_l;
    let bias_r = tau_y_cl_r - tau_y_bc_r;

    // ----- 11. Residuals -----
    let predicts_p_l = &r_p_l * &beta_p_l_mat;
    let predicts_p_r = &r_p_r * &beta_p_r_mat;
    let predicts_q_l = &r_q_l * &beta_q_l_mat;
    let predicts_q_r = &r_q_r * &beta_q_r_mat;

    let hii_p_l = if vce == "hc2" || vce == "hc3" {
        hat_values(&r_p_l, &inv_g_p_l, &w_h_l_eff)
    } else {
        vec![0.0; en_l]
    };
    let hii_p_r = if vce == "hc2" || vce == "hc3" {
        hat_values(&r_p_r, &inv_g_p_r, &w_h_r_eff)
    } else {
        vec![0.0; en_r]
    };

    let predicts_p_l_col: Vec<f64> = (0..en_l).map(|i| predicts_p_l[(i, 0)]).collect();
    let predicts_p_r_col: Vec<f64> = (0..en_r).map(|i| predicts_p_r[(i, 0)]).collect();
    let m_p_l = Mat::from_fn(en_l, 1, |i, _| predicts_p_l_col[i]);
    let m_p_r = Mat::from_fn(en_r, 1, |i, _| predicts_p_r_col[i]);

    let ec_l: Option<Vec<f64>> = c_l
        .as_ref()
        .map(|cl| e_idx_l.iter().map(|&i| cl[i]).collect());
    let ec_r: Option<Vec<f64>> = c_r
        .as_ref()
        .map(|cl| e_idx_r.iter().map(|&i| cl[i]).collect());

    let res_h_l = rdrobust_res(
        &e_x_l,
        &e_y_l,
        None,
        None,
        &m_p_l,
        &hii_p_l,
        vce,
        cfg.nnmatch,
        &edups_l,
        &edupsid_l,
        p + 1,
        crv3,
        crv2,
        ec_l.is_some(),
    );
    let res_h_r = rdrobust_res(
        &e_x_r,
        &e_y_r,
        None,
        None,
        &m_p_r,
        &hii_p_r,
        vce,
        cfg.nnmatch,
        &edups_r,
        &edupsid_r,
        p + 1,
        crv3,
        crv2,
        ec_r.is_some(),
    );

    let res_b_l = if vce == "nn" {
        res_h_l.clone()
    } else {
        let hii_q_l = if vce == "hc2" || vce == "hc3" {
            hat_values(&r_q_l, &inv_g_q_l, &w_b_l_eff)
        } else {
            vec![0.0; en_l]
        };
        let predicts_q_l_col: Vec<f64> = (0..en_l).map(|i| predicts_q_l[(i, 0)]).collect();
        let m_q_l = Mat::from_fn(en_l, 1, |i, _| predicts_q_l_col[i]);
        rdrobust_res(
            &e_x_l,
            &e_y_l,
            None,
            None,
            &m_q_l,
            &hii_q_l,
            vce,
            cfg.nnmatch,
            &edups_l,
            &edupsid_l,
            q + 1,
            crv3,
            crv2,
            ec_l.is_some(),
        )
    };
    let res_b_r = if vce == "nn" {
        res_h_r.clone()
    } else {
        let hii_q_r = if vce == "hc2" || vce == "hc3" {
            hat_values(&r_q_r, &inv_g_q_r, &w_b_r_eff)
        } else {
            vec![0.0; en_r]
        };
        let predicts_q_r_col: Vec<f64> = (0..en_r).map(|i| predicts_q_r[(i, 0)]).collect();
        let m_q_r = Mat::from_fn(en_r, 1, |i, _| predicts_q_r_col[i]);
        rdrobust_res(
            &e_x_r,
            &e_y_r,
            None,
            None,
            &m_q_r,
            &hii_q_r,
            vce,
            cfg.nnmatch,
            &edups_r,
            &edupsid_r,
            q + 1,
            crv3,
            crv2,
            ec_r.is_some(),
        )
    };

    // ----- 12. Variance matrices -----
    let s_y = vec![1.0]; // sharp RD, no covs

    // V_cl: invG_p * vce(R_p * W_h, res_h) * invG_p
    let r_p_wh_l = scale_rows(&r_p_l, &w_h_l_eff);
    let r_p_wh_r = scale_rows(&r_p_r, &w_h_r_eff);

    let cidx_l: Option<Vec<Vec<usize>>> = ec_l.as_ref().map(|c| cluster_idx(c));
    let cidx_r: Option<Vec<Vec<usize>>> = ec_r.as_ref().map(|c| cluster_idx(c));

    let sqrt_rx_p_l: Option<Mat<f64>> = if crv3 || crv2 {
        Some(scale_rows(&r_p_l, &sqrt_wh_l))
    } else {
        None
    };
    let sqrt_rx_p_r: Option<Mat<f64>> = if crv3 || crv2 {
        Some(scale_rows(&r_p_r, &sqrt_wh_r))
    } else {
        None
    };

    let crv_inv_g_l = if crv3 || crv2 { Some(&inv_g_p_l) } else { None };
    let crv_inv_g_r = if crv3 || crv2 { Some(&inv_g_p_r) } else { None };

    let v_cl_l = {
        let meat = rdrobust_vce(
            0,
            &s_y,
            &r_p_wh_l,
            &res_h_l,
            ec_l.as_deref(),
            cidx_l.as_deref(),
            crv_inv_g_l,
            sqrt_rx_p_l.as_ref(),
            crv2,
            None,
        );
        &inv_g_p_l * &meat * &inv_g_p_l
    };
    let v_cl_r = {
        let meat = rdrobust_vce(
            0,
            &s_y,
            &r_p_wh_r,
            &res_h_r,
            ec_r.as_deref(),
            cidx_r.as_deref(),
            crv_inv_g_r,
            sqrt_rx_p_r.as_ref(),
            crv2,
            None,
        );
        &inv_g_p_r * &meat * &inv_g_p_r
    };

    // V_rb: when h==b, use q-regression directly. Otherwise use Q_q.
    let hb_match = (h_l == b_l) && (h_r == b_r);

    let v_rb_l = if hb_match && (crv3 || crv2) {
        let r_q_wh_l = scale_rows(&r_q_l, &w_h_l_eff);
        let sqrt_rx_q_l = scale_rows(&r_q_l, &sqrt_wh_l);
        let meat = rdrobust_vce(
            0,
            &s_y,
            &r_q_wh_l,
            &res_b_l,
            ec_l.as_deref(),
            cidx_l.as_deref(),
            Some(&inv_g_q_l),
            Some(&sqrt_rx_q_l),
            crv2,
            None,
        );
        &inv_g_q_l * &meat * &inv_g_q_l
    } else if hb_match && cluster.is_some() {
        let r_q_wh_l = scale_rows(&r_q_l, &w_h_l_eff);
        let meat = rdrobust_vce(
            0,
            &s_y,
            &r_q_wh_l,
            &res_b_l,
            ec_l.as_deref(),
            cidx_l.as_deref(),
            None,
            None,
            false,
            None,
        );
        &inv_g_q_l * &meat * &inv_g_q_l
    } else {
        let meat = rdrobust_vce(
            0,
            &s_y,
            &q_q_l,
            &res_b_l,
            ec_l.as_deref(),
            cidx_l.as_deref(),
            None,
            None,
            false,
            Some(q + 1),
        );
        &inv_g_p_l * &meat * &inv_g_p_l
    };

    let v_rb_r = if hb_match && (crv3 || crv2) {
        let r_q_wh_r = scale_rows(&r_q_r, &w_h_r_eff);
        let sqrt_rx_q_r = scale_rows(&r_q_r, &sqrt_wh_r);
        let meat = rdrobust_vce(
            0,
            &s_y,
            &r_q_wh_r,
            &res_b_r,
            ec_r.as_deref(),
            cidx_r.as_deref(),
            Some(&inv_g_q_r),
            Some(&sqrt_rx_q_r),
            crv2,
            None,
        );
        &inv_g_q_r * &meat * &inv_g_q_r
    } else if hb_match && cluster.is_some() {
        let r_q_wh_r = scale_rows(&r_q_r, &w_h_r_eff);
        let meat = rdrobust_vce(
            0,
            &s_y,
            &r_q_wh_r,
            &res_b_r,
            ec_r.as_deref(),
            cidx_r.as_deref(),
            None,
            None,
            false,
            None,
        );
        &inv_g_q_r * &meat * &inv_g_q_r
    } else {
        let meat = rdrobust_vce(
            0,
            &s_y,
            &q_q_r,
            &res_b_r,
            ec_r.as_deref(),
            cidx_r.as_deref(),
            None,
            None,
            false,
            Some(q + 1),
        );
        &inv_g_p_r * &meat * &inv_g_p_r
    };

    // se_tau
    let se_cl = (scalepar
        * scalepar
        * fact_deriv
        * fact_deriv
        * (v_cl_l[(deriv, deriv)] + v_cl_r[(deriv, deriv)]))
        .sqrt();
    let se_rb = (scalepar
        * scalepar
        * fact_deriv
        * fact_deriv
        * (v_rb_l[(deriv, deriv)] + v_rb_r[(deriv, deriv)]))
        .sqrt();

    // ----- 13. Inference -----
    let normal = Normal::new(0.0, 1.0).unwrap();
    let tau_arr = [tau_cl, tau_bc, tau_bc];
    let se_arr = [se_cl, se_cl, se_rb];
    let z_arr = [
        tau_arr[0] / se_arr[0],
        tau_arr[1] / se_arr[1],
        tau_arr[2] / se_arr[2],
    ];
    let pv_arr = z_arr.map(|z| 2.0 * normal.cdf(-z.abs()));
    let ci_arr = [
        [
            tau_arr[0] - quant * se_arr[0],
            tau_arr[0] + quant * se_arr[0],
        ],
        [
            tau_arr[1] - quant * se_arr[1],
            tau_arr[1] + quant * se_arr[1],
        ],
        [
            tau_arr[2] - quant * se_arr[2],
            tau_arr[2] + quant * se_arr[2],
        ],
    ];

    // Beta coefficients for output (per-side, scalepar * factorial(deriv))
    let beta_y_p_l: Vec<f64> = (0..=p)
        .map(|i| scalepar * fact_deriv * beta_p_l_mat[(i, 0)])
        .collect();
    let beta_y_p_r: Vec<f64> = (0..=p)
        .map(|i| scalepar * fact_deriv * beta_p_r_mat[(i, 0)])
        .collect();

    Ok(RdRobustOutput {
        tau_cl,
        tau_bc,
        se_cl,
        se_rb,
        z: z_arr,
        pv: pv_arr,
        ci: ci_arr,
        h_l,
        h_r,
        b_l,
        b_r,
        n_l,
        n_r,
        n_h_l,
        n_h_r,
        n_b_l,
        n_b_r,
        bias_l,
        bias_r,
        v_cl_l,
        v_cl_r,
        v_rb_l,
        v_rb_r,
        kernel: kernel.label().to_string(),
        vce_type: vce_type.to_string(),
        bwselect: bwselect_lower.clone(),
        p,
        q,
        deriv,
        level,
        tau_cl_l: tau_y_cl_l,
        tau_cl_r: tau_y_cl_r,
        tau_bc_l: tau_y_bc_l,
        tau_bc_r: tau_y_bc_r,
        beta_y_p_l,
        beta_y_p_r,
    })
}

// =====================================================================
// Helpers
// =====================================================================

fn sd(v: &[f64]) -> f64 {
    let n = v.len() as f64;
    let mean = v.iter().sum::<f64>() / n;
    let var = v.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / (n - 1.0);
    var.sqrt()
}

fn iqr(v: &[f64]) -> f64 {
    let mut sorted = v.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let n = sorted.len();
    let q25 = sorted[(0.25 * n as f64).floor() as usize];
    let q75 = sorted[(0.75 * n as f64).floor() as usize];
    q75 - q25
}

fn median(v: &[f64]) -> f64 {
    let mut sorted = v.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let n = sorted.len();
    if n % 2 == 0 {
        (sorted[n / 2 - 1] + sorted[n / 2]) / 2.0
    } else {
        sorted[n / 2]
    }
}

/// Build the bias-corrected design matrix Q_q.
///
/// R: `Q_q = t(t(R_p*W_h) - h^(p+1)*(L%*%t(e_p1))%*%t(t(invG_q%*%t(R_q))*W_b))`
///
/// Q_q[i,j] = R_p[i,j]*W_h[i] - h^(p+1) * L[j] * (R_q %*% invG_q[:,p+1])[i] * W_b[i]
#[allow(clippy::too_many_arguments)]
fn build_qq(
    r_p: &Mat<f64>,
    w_h: &[f64],
    r_q: &Mat<f64>,
    w_b: &[f64],
    inv_g_q: &Mat<f64>,
    l: &Mat<f64>,
    h: f64,
    p: usize,
    _q: usize,
) -> Mat<f64> {
    let n = r_p.nrows();
    let ncols = p + 1;
    let k_q = r_q.ncols();

    // Compute R_q %*% invG_q[:,p+1] (column p+1 of invG_q, 0-based)
    // This gives a n×1 vector
    let mut rq_invg_col = vec![0.0; n]; // R_q %*% invG_q[:,p+1]
    for i in 0..n {
        let mut s = 0.0;
        for k in 0..k_q {
            s += r_q[(i, k)] * inv_g_q[(k, p + 1)];
        }
        rq_invg_col[i] = s;
    }

    let h_pp1 = h.powi((p + 1) as i32);
    let mut out = Mat::zeros(n, ncols);
    for i in 0..n {
        for j in 0..ncols {
            out[(i, j)] = r_p[(i, j)] * w_h[i] - h_pp1 * l[(j, 0)] * rq_invg_col[i] * w_b[i];
        }
    }
    out
}
