//! Sensitivity analysis for unmeasured confounding in meta-analyses.
//!
//! Port of `R/meta-analysis.R` — `confounded_meta()` function.

use crate::bootstrap::bca_ci;
use crate::calib::calib_ests;
use crate::error::{EvalueError, Result};
use crate::math_utils::g;
use statrs::distribution::{ContinuousCDF, Normal};

/// Output row from `confounded_meta()`.
#[derive(Clone, Debug)]
pub struct ConfoundedMetaRow {
    pub value: &'static str,
    pub est: f64,
    pub se: Option<f64>,
    pub ci_lo: Option<f64>,
    pub ci_hi: Option<f64>,
}

/// Configuration for `confounded_meta()`.
#[derive(Clone, Debug)]
pub struct MetaConfig {
    pub method: MetaMethod,
    pub q: f64,
    pub r: Option<f64>,
    pub tail: Option<Tail>,
    pub ci_level: f64,
    pub give_ci: bool,
    pub r_boot: usize,
    pub mu_b: Option<f64>,
    pub mu_b_toward_null: bool,
    pub seed: u64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum MetaMethod {
    Calibrated,
    Parametric,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Tail {
    Above,
    Below,
}

impl Default for MetaConfig {
    fn default() -> Self {
        Self {
            method: MetaMethod::Calibrated,
            q: 0.0,
            r: None,
            tail: None,
            ci_level: 0.95,
            give_ci: true,
            r_boot: 1000,
            mu_b: None,
            mu_b_toward_null: false,
            seed: 42,
        }
    }
}

/// Calibrated method data input.
#[derive(Clone, Debug)]
pub struct CalibratedData {
    pub yi: Vec<f64>,
    pub vi: Vec<f64>,
}

/// Parametric method input.
#[derive(Clone, Debug)]
pub struct ParametricInput {
    pub yr: f64,
    pub vyr: Option<f64>,
    pub t2: f64,
    pub vt2: Option<f64>,
    pub sig_b: f64,
}

/// Run `confounded_meta()` — sensitivity analysis for meta-analyses.
///
/// Port of `confounded_meta()` in `meta-analysis.R:202`.
pub fn confounded_meta(
    cfg: &MetaConfig,
    cal: Option<&CalibratedData>,
    par: Option<&ParametricInput>,
) -> Result<Vec<ConfoundedMetaRow>> {
    match cfg.method {
        MetaMethod::Parametric => confounded_meta_parametric(cfg, par),
        MetaMethod::Calibrated => confounded_meta_calibrated(cfg, cal),
    }
}

fn confounded_meta_parametric(
    cfg: &MetaConfig,
    par: Option<&ParametricInput>,
) -> Result<Vec<ConfoundedMetaRow>> {
    let par = par.ok_or_else(|| {
        EvalueError::Invalid("Parametric method requires ParametricInput".into())
    })?;

    if par.t2 < 0.0 {
        return Err(EvalueError::Invalid("Heterogeneity cannot be negative".into()));
    }

    let z = Normal::new(0.0, 1.0).unwrap();

    // Determine tail
    let tail = cfg.tail.unwrap_or(if par.yr > 0.0 {
        Tail::Above
    } else {
        Tail::Below
    });

    let mu_b = cfg.mu_b.unwrap_or(0.0);

    // Bias-corrected mean
    let yr_corr = if !cfg.mu_b_toward_null {
        if par.yr > 0.0 {
            par.yr - mu_b
        } else {
            par.yr + mu_b
        }
    } else {
        if par.yr > 0.0 {
            par.yr + mu_b
        } else {
            par.yr - mu_b
        }
    };

    let denom = (par.t2 - par.sig_b * par.sig_b).max(1e-15);

    // Phat (proportion)
    let phat = if cfg.mu_b.is_some() {
        match tail {
            Tail::Above => {
                let z_val = (cfg.q - yr_corr) / denom.sqrt();
                1.0 - z.cdf(z_val)
            }
            Tail::Below => {
                let z_val = (cfg.q - yr_corr) / denom.sqrt();
                z.cdf(z_val)
            }
        }
    } else {
        f64::NAN
    };

    // Tmin, Gmin
    let (tmin, gmin) = if let Some(r_val) = cfg.r {
        let phat_naive = match tail {
            Tail::Above => 1.0 - z.cdf((cfg.q - par.yr) / denom.sqrt()),
            Tail::Below => z.cdf((cfg.q - par.yr) / denom.sqrt()),
        };

        if phat_naive <= r_val {
            (1.0_f64, 1.0_f64)
        } else {
            let t = match tail {
                Tail::Above => {
                    1.0_f64.max((z.inverse_cdf(1.0 - r_val) * denom.sqrt() - cfg.q + par.yr).exp())
                }
                Tail::Below => {
                    1.0_f64.max((cfg.q - par.yr - z.inverse_cdf(r_val) * denom.sqrt()).exp())
                }
            };
            let gmin = t + (t * t - t).max(0.0).sqrt();
            (t, gmin)
        }
    } else {
        (f64::NAN, f64::NAN)
    };

    // Delta method inference for Phat
    let (se_phat, lo_phat, hi_phat) = if let (Some(vyr), Some(vt2), Some(_)) =
        (par.vyr, par.vt2, cfg.mu_b)
    {
        let num_term = match tail {
            Tail::Above => cfg.q + mu_b - par.yr,
            Tail::Below => cfg.q - mu_b - par.yr,
        };

        let term1_1 = vyr / denom;
        let term1_2 = vt2 * num_term * num_term / (4.0 * denom.powi(3));
        let term1 = (term1_1 + term1_2).sqrt();
        let z_val = num_term / denom.sqrt();
        // se = term1 * dnorm(Z)
        let se_phat = term1 * (-z_val * z_val / 2.0).exp() / (2.0 * std::f64::consts::PI).sqrt();

        let tail_prob = (1.0 - cfg.ci_level) / 2.0;
        let lo = (phat + z.inverse_cdf(tail_prob) * se_phat).max(0.0);
        let hi = (phat - z.inverse_cdf(tail_prob) * se_phat).min(1.0);
        (se_phat, lo, hi)
    } else {
        (f64::NAN, f64::NAN, f64::NAN)
    };

    // Delta method for Tmin, Gmin
    let (se_t, lo_t, hi_t, se_g, lo_g, hi_g) =
        if let (Some(vyr), Some(vt2), Some(r_val)) = (par.vyr, par.vt2, cfg.r) {
            if tmin != 1.0 && tmin.is_finite() {
                let term = match tail {
                    Tail::Above => {
                        vt2 * z.inverse_cdf(1.0 - r_val).powi(2) / (4.0 * denom)
                    }
                    Tail::Below => {
                        vt2 * z.inverse_cdf(r_val).powi(2) / (4.0 * denom)
                    }
                };
                let se_t = match tail {
                    Tail::Above => {
                        (z.inverse_cdf(1.0 - r_val) * denom.sqrt() - cfg.q + par.yr).exp()
                            * (vyr + term).sqrt()
                    }
                    Tail::Below => {
                        (cfg.q - par.yr - z.inverse_cdf(r_val) * denom.sqrt()).exp()
                            * (vyr + term).sqrt()
                    }
                };

                let tail_prob = (1.0 - cfg.ci_level) / 2.0;
                let lo_t = (tmin + z.inverse_cdf(tail_prob) * se_t).max(1.0);
                let hi_t = tmin - z.inverse_cdf(tail_prob) * se_t;

                let se_g = se_t * (1.0 + (2.0 * tmin - 1.0) / (2.0 * (tmin * tmin - tmin).max(1e-15).sqrt()));
                let lo_g = (gmin + z.inverse_cdf(tail_prob) * se_g).max(1.0);
                let hi_g = gmin - z.inverse_cdf(tail_prob) * se_g;

                (se_t, lo_t, hi_t, se_g, lo_g, hi_g)
            } else {
                (f64::NAN, f64::NAN, f64::NAN, f64::NAN, f64::NAN, f64::NAN)
            }
        } else {
            (f64::NAN, f64::NAN, f64::NAN, f64::NAN, f64::NAN, f64::NAN)
        };

    Ok(vec![
        ConfoundedMetaRow {
            value: "Prop",
            est: phat,
            se: if se_phat.is_nan() { None } else { Some(se_phat) },
            ci_lo: if lo_phat.is_nan() { None } else { Some(lo_phat) },
            ci_hi: if hi_phat.is_nan() { None } else { Some(hi_phat) },
        },
        ConfoundedMetaRow {
            value: "Tmin",
            est: tmin,
            se: if se_t.is_nan() { None } else { Some(se_t) },
            ci_lo: if lo_t.is_nan() { None } else { Some(lo_t) },
            ci_hi: if hi_t.is_nan() { None } else { Some(hi_t) },
        },
        ConfoundedMetaRow {
            value: "Gmin",
            est: gmin,
            se: if se_g.is_nan() { None } else { Some(se_g) },
            ci_lo: if lo_g.is_nan() { None } else { Some(lo_g) },
            ci_hi: if hi_g.is_nan() { None } else { Some(hi_g) },
        },
    ])
}

fn confounded_meta_calibrated(
    cfg: &MetaConfig,
    cal: Option<&CalibratedData>,
) -> Result<Vec<ConfoundedMetaRow>> {
    let cal = cal.ok_or_else(|| {
        EvalueError::Invalid("Calibrated method requires CalibratedData".into())
    })?;

    let sei: Vec<f64> = cal.vi.iter().map(|v| v.sqrt()).collect();
    let calib = calib_ests(&cal.yi, &sei);

    // Determine tail
    let tail = cfg.tail.unwrap_or({
        let median = median(&calib);
        if median > 0.0 {
            Tail::Above
        } else {
            Tail::Below
        }
    });

    let mu_b = cfg.mu_b.unwrap_or(0.0);

    // Phat point estimate
    let phat = phat_causal(&calib, mu_b, tail, cfg.mu_b_toward_null, cfg.q);

    // Tmin, Gmin
    let (tmin, gmin) = if let Some(r_val) = cfg.r {
        let t = tmin_causal(&calib, cfg.q, r_val, tail);
        (t, g(t))
    } else {
        (f64::NAN, f64::NAN)
    };

    // Convert to study-level tuples for bootstrap
    let studies: Vec<(f64, f64)> = cal.yi.iter().zip(cal.vi.iter()).map(|(&y, &v)| (y, v)).collect();

    // CI via bootstrap
    let (se_phat, lo_phat, hi_phat) = if cfg.give_ci {
        let result = bca_ci(
            &studies,
            |data| {
                let yi: Vec<f64> = data.iter().map(|(y, _)| *y).collect();
                let vi: Vec<f64> = data.iter().map(|(_, v)| *v).collect();
                let s: Vec<f64> = vi.iter().map(|v| v.sqrt()).collect();
                let cb = calib_ests(&yi, &s);
                phat_causal(&cb, mu_b, tail, cfg.mu_b_toward_null, cfg.q)
            },
            cfg.r_boot,
            cfg.ci_level,
            cfg.seed,
        );
        (result.se, result.lo, result.hi)
    } else {
        (f64::NAN, None, None)
    };

    // CI for Tmin/Gmin via bootstrap
    let (se_t, lo_t, hi_t, se_g, lo_g, hi_g) = if cfg.give_ci && cfg.r.is_some() && tmin.is_finite() && tmin != 1.0 {
        let r_val = cfg.r.unwrap();
        let result = bca_ci(
            &studies,
            |data| {
                let yi: Vec<f64> = data.iter().map(|(y, _)| *y).collect();
                let vi: Vec<f64> = data.iter().map(|(_, v)| *v).collect();
                let s: Vec<f64> = vi.iter().map(|v| v.sqrt()).collect();
                let cb = calib_ests(&yi, &s);
                tmin_causal(&cb, cfg.q, r_val, tail)
            },
            cfg.r_boot,
            cfg.ci_level,
            cfg.seed + 1,
        );
        let lo_t = result.lo.map(|v| v.max(1.0));
        let hi_t = result.hi;
        let se_t = result.se;
        let boot_g: Vec<f64> = result.boot_vals.iter().map(|&v| g(v)).collect();
        let se_g = {
            let mean: f64 = boot_g.iter().sum::<f64>() / boot_g.len().max(1) as f64;
            let var: f64 = boot_g.iter().map(|v| (v - mean).powi(2)).sum::<f64>()
                / boot_g.len().max(1) as f64;
            var.sqrt()
        };
        let lo_g = lo_t.map(g);
        let hi_g = hi_t.map(g);
        (se_t, lo_t, hi_t, se_g, lo_g, hi_g)
    } else {
        (f64::NAN, None, None, f64::NAN, None, None)
    };

    Ok(vec![
        ConfoundedMetaRow {
            value: "Prop",
            est: phat,
            se: if se_phat.is_nan() { None } else { Some(se_phat) },
            ci_lo: lo_phat,
            ci_hi: hi_phat,
        },
        ConfoundedMetaRow {
            value: "Tmin",
            est: tmin,
            se: if se_t.is_nan() { None } else { Some(se_t) },
            ci_lo: lo_t,
            ci_hi: hi_t,
        },
        ConfoundedMetaRow {
            value: "Gmin",
            est: gmin,
            se: if se_g.is_nan() { None } else { Some(se_g) },
            ci_lo: lo_g,
            ci_hi: hi_g,
        },
    ])
}

/// Proportion of studies with causal effects above/below q.
///
/// Port of `Phat_causal()` in `meta-analysis.R:1067`.
fn phat_causal(
    calib: &[f64],
    b: f64,
    tail: Tail,
    mu_b_toward_null: bool,
    q: f64,
) -> f64 {
    let median_calib = median(calib);

    let calib_t: Vec<f64> = calib
        .iter()
        .map(|&c| {
            if median_calib > 0.0 && !mu_b_toward_null {
                c - b
            } else if median_calib < 0.0 && !mu_b_toward_null {
                c + b
            } else if median_calib > 0.0 && mu_b_toward_null {
                c + b
            } else {
                c - b
            }
        })
        .collect();

    match tail {
        Tail::Above => calib_t.iter().filter(|&&v| v > q).count() as f64 / calib_t.len() as f64,
        Tail::Below => calib_t.iter().filter(|&&v| v < q).count() as f64 / calib_t.len() as f64,
    }
}

/// Minimum common bias factor.
///
/// Port of `Tmin_causal()` in `meta-analysis.R:1103`.
fn tmin_causal(calib: &[f64], q: f64, r: f64, tail: Tail) -> f64 {
    let phat_c = phat_causal(calib, 0.0, tail, false, q);
    if phat_c <= r {
        return 1.0;
    }

    let mut sorted: Vec<f64> = calib.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

    // ECDF values
    let n = sorted.len();
    let phat_options: Vec<f64> = (0..n).map(|i| (i + 1) as f64 / n as f64).collect();

    // Add 0 as always possible
    let mut all_options = phat_options.clone();
    all_options.push(0.0);

    // Of phats <= r, find the largest
    let phat_target = all_options
        .iter()
        .filter(|&&p| p <= r)
        .cloned()
        .fold(0.0_f64, f64::max);

    let k = n as f64;

    let calib_star = match tail {
        Tail::Above => {
            let idx = (k - k * phat_target).round() as usize;
            sorted[idx.min(n - 1)]
        }
        Tail::Below => {
            let idx = (k * phat_target + 1.0).round() as usize;
            sorted[idx.min(n - 1)]
        }
    };

    (calib_star - q).abs().exp() + 0.001
}

fn median(v: &[f64]) -> f64 {
    if v.is_empty() {
        return 0.0;
    }
    let mut s = v.to_vec();
    s.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let n = s.len();
    if n % 2 == 0 {
        (s[n / 2 - 1] + s[n / 2]) / 2.0
    } else {
        s[n / 2]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parametric_basic() {
        let cfg = MetaConfig {
            method: MetaMethod::Parametric,
            q: (0.9_f64).ln(),
            r: Some(0.20),
            tail: Some(Tail::Below),
            mu_b: Some((1.5_f64).ln()),
            ..Default::default()
        };
        let par = ParametricInput {
            yr: (1.3_f64).ln(),
            vyr: Some(0.05),
            t2: 0.4,
            vt2: Some(0.001),
            sig_b: 0.0,
        };
        let rows = confounded_meta(&cfg, None, Some(&par)).unwrap();
        assert_eq!(rows.len(), 3);
        // Prop should be between 0 and 1
        assert!(rows[0].est >= 0.0 && rows[0].est <= 1.0);
        // Tmin and Gmin should be >= 1
        assert!(rows[1].est >= 1.0 || rows[1].est.is_nan());
    }

    #[test]
    fn test_calibrated_basic() {
        // Simple toy meta-analysis
        let yi: Vec<f64> = (0..20)
            .map(|i| (-0.5 + i as f64 * 0.1).ln())
            .collect();
        let vi: Vec<f64> = vec![0.05; 20];

        let cfg = MetaConfig {
            method: MetaMethod::Calibrated,
            q: (0.9_f64).ln(),
            tail: Some(Tail::Below),
            mu_b: Some((1.5_f64).ln()),
            give_ci: false,
            ..Default::default()
        };
        let cal = CalibratedData { yi, vi };
        let rows = confounded_meta(&cfg, Some(&cal), None).unwrap();
        assert_eq!(rows.len(), 3);
        assert!(rows[0].est >= 0.0 && rows[0].est <= 1.0);
    }
}
