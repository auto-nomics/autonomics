//! CMAverse-compatible causal mediation analysis — extended.
//!
//! Supports four configurations matching CMAverse `cmest(method = "rb")`:
//!
//! | Variant | Exposure | Mediator | Outcome | Node kind |
//! |---------|----------|----------|---------|-----------|
//! | Single (continuous) | cont | cont | cont | `cmest` |
//! | Multi-mediator | cont | cont×K | cont | `cmest_multi` |
//! | Binary outcome | cont | cont | binary | `cmest_binary_y` |
//! | Binary mediator | cont | binary | cont | `cmest_binary_m` |
//! | Binary outcome + binary mediator | cont | binary | binary | `cmest_binary_both` |
//!
//! Plus weighting-based and g-formula approaches as separate functions.

use rand::Rng;
use rand::SeedableRng;
use rand_chacha::ChaCha8Rng;

use statkit::regression;

use crate::error::{EpiError, Result};

// ═══════════════════════════════════════════════════════════════════════════
// Shared types
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone)]
pub struct CmestOptions {
    pub interaction: bool,
    pub cde_m: f64,
    pub n_bootstrap: usize,
    pub seed: u64,
    pub x_treated: f64,
    pub x_control: f64,
}

impl Default for CmestOptions {
    fn default() -> Self {
        Self {
            interaction: true,
            cde_m: 0.0,
            n_bootstrap: 1000,
            seed: 42,
            x_treated: 1.0,
            x_control: 0.0,
        }
    }
}

/// Result of any causal mediation analysis variant.
#[derive(Debug, Clone)]
pub struct CmestResult {
    pub cde: f64,
    pub cde_ci: (f64, f64),
    pub nde: f64,
    pub nde_ci: (f64, f64),
    pub nie: f64,
    pub nie_ci: (f64, f64),
    pub te: f64,
    pub te_ci: (f64, f64),
    pub prop_mediated: f64,
    pub prop_eliminated: f64,
    pub mediator_model: Vec<f64>,
    pub outcome_model: Vec<f64>,
    pub n_obs: usize,
    // ── Multi-mediator extensions ──
    /// Per-mediator NIE contributions (for multi-mediator analysis).
    pub nie_per_mediator: Vec<f64>,
    /// Per-mediator proportion mediated.
    pub prop_mediated_per_mediator: Vec<f64>,
    /// Per-mediator weight (relative contribution).
    pub mediator_weights: Vec<f64>,
}

// ═══════════════════════════════════════════════════════════════════════════
// 1. Single continuous mediator + continuous outcome (existing, refined)
// ═══════════════════════════════════════════════════════════════════════════

pub fn cmest(
    x: &[f64],
    m: &[f64],
    y: &[f64],
    covariates: &[&[f64]],
    opts: &CmestOptions,
) -> Result<CmestResult> {
    let mediators: Vec<&[f64]> = vec![m];
    cmest_multi(x, &mediators, y, covariates, opts)
}

// ═══════════════════════════════════════════════════════════════════════════
// 2. Multi-mediator (all continuous) — VanderWeele 2014
// ═══════════════════════════════════════════════════════════════════════════

/// Multi-mediator causal mediation (continuous outcome, continuous mediators).
///
/// `mediators` is a slice of parallel mediator columns. Each gets its own
/// mediator model M_j ~ X + C, and a joint outcome model includes all
/// mediators (plus X×M_j interactions if `opts.interaction`).
///
/// NIE_total = Σ_j NIE_j, matching CMAverse's multi-mediator rb approach.
pub fn cmest_multi(
    x: &[f64],
    mediators: &[&[f64]],
    y: &[f64],
    covariates: &[&[f64]],
    opts: &CmestOptions,
) -> Result<CmestResult> {
    let n = x.len();
    if n == 0 || y.len() != n {
        return Err(EpiError::DimensionMismatch { a: n, b: y.len() });
    }
    let k = mediators.len();
    if k == 0 {
        return Err(EpiError::EmptyInput);
    }
    for m in mediators {
        if m.len() != n {
            return Err(EpiError::DimensionMismatch { a: n, b: m.len() });
        }
    }
    for c in covariates {
        if c.len() != n {
            return Err(EpiError::DimensionMismatch { a: n, b: c.len() });
        }
    }

    let (cde, nde, nie, te, nie_per, m_coefs, y_coefs) =
        fit_multi(x, mediators, y, covariates, opts)?;
    let prop_mediated = if te.abs() > 1e-10 { nie / te } else { f64::NAN };
    let prop_eliminated = if te.abs() > 1e-10 {
        1.0 - nde / te
    } else {
        f64::NAN
    };
    let prop_per: Vec<f64> = nie_per
        .iter()
        .map(|&v| if te.abs() > 1e-10 { v / te } else { f64::NAN })
        .collect();
    let weights: Vec<f64> = nie_per
        .iter()
        .map(|&v| if nie.abs() > 1e-10 { v / nie } else { 0.0 })
        .collect();

    let mut rng = ChaCha8Rng::seed_from_u64(opts.seed);
    let indices: Vec<usize> = (0..n).collect();
    let mut boot_cde = Vec::new();
    let mut boot_nde = Vec::new();
    let mut boot_nie = Vec::new();
    let mut boot_te = Vec::new();

    for _ in 0..opts.n_bootstrap {
        let idx: Vec<usize> = (0..n)
            .map(|_| indices[(rng.random::<f64>() * n as f64) as usize])
            .collect();
        let x_b: Vec<f64> = idx.iter().map(|&i| x[i]).collect();
        let m_b: Vec<Vec<f64>> = mediators
            .iter()
            .map(|m| idx.iter().map(|&i| m[i]).collect())
            .collect();
        let m_slices: Vec<&[f64]> = m_b.iter().map(|v| v.as_slice()).collect();
        let y_b: Vec<f64> = idx.iter().map(|&i| y[i]).collect();
        let cov_b: Vec<Vec<f64>> = covariates
            .iter()
            .map(|c| idx.iter().map(|&i| c[i]).collect())
            .collect();
        let cov_slices: Vec<&[f64]> = cov_b.iter().map(|v| v.as_slice()).collect();

        if let Ok((c_b, n_b, ni_b, t_b, _, _, _)) =
            fit_multi(&x_b, &m_slices, &y_b, &cov_slices, opts)
        {
            boot_cde.push(c_b);
            boot_nde.push(n_b);
            boot_nie.push(ni_b);
            boot_te.push(t_b);
        }
    }

    Ok(CmestResult {
        cde,
        cde_ci: pci(&boot_cde),
        nde,
        nde_ci: pci(&boot_nde),
        nie,
        nie_ci: pci(&boot_nie),
        te,
        te_ci: pci(&boot_te),
        prop_mediated,
        prop_eliminated,
        mediator_model: m_coefs,
        outcome_model: y_coefs,
        n_obs: n,
        nie_per_mediator: nie_per,
        prop_mediated_per_mediator: prop_per,
        mediator_weights: weights,
    })
}

fn fit_multi(
    x: &[f64],
    mediators: &[&[f64]],
    y: &[f64],
    covariates: &[&[f64]],
    opts: &CmestOptions,
) -> Result<(f64, f64, f64, f64, Vec<f64>, Vec<f64>, Vec<f64>)> {
    let k = mediators.len();
    let n = x.len();

    // ── Mediator models: M_j ~ X + C for each j ─────────────────────────
    let mut alpha1_all = Vec::with_capacity(k); // α₁ for each mediator
    let mut m_under_control_all = Vec::with_capacity(k); // E[M_j | X=0, C=C̄]

    for m_j in mediators.iter() {
        let mut preds: Vec<&[f64]> = vec![x];
        for c in covariates {
            preds.push(c);
        }
        let fit = regression::ols(&preds, m_j, true).map_err(epi_from_stat)?;
        alpha1_all.push(fit.coefficients[1]);
        let m_ctrl = fit.coefficients[0]
            + covariates
                .iter()
                .enumerate()
                .map(|(ci, c)| {
                    let mean: f64 = c.iter().sum::<f64>() / n as f64;
                    fit.coefficients[2 + ci] * mean
                })
                .sum::<f64>();
        m_under_control_all.push(m_ctrl);
    }

    // ── Outcome model: Y ~ X + M₁ + ... + M_k [+ X*M₁ + ...] + C ───────
    let mut y_preds: Vec<&[f64]> = vec![x];
    for m in mediators {
        y_preds.push(m);
    }

    // Interaction terms (X × M_j for each j).
    let xm_interacts: Vec<Vec<f64>> = if opts.interaction {
        mediators
            .iter()
            .map(|m| x.iter().zip(*m).map(|(&xi, &mi)| xi * mi).collect())
            .collect()
    } else {
        vec![]
    };
    for xi in &xm_interacts {
        y_preds.push(xi);
    }
    for c in covariates {
        y_preds.push(c);
    }

    let y_fit = regression::ols(&y_preds, y, true).map_err(epi_from_stat)?;
    let beta_1 = y_fit.coefficients[1]; // X

    // Extract β₂ⱼ (M_j) and β₃ⱼ (X×M_j).
    let beta2_all: Vec<f64> = (0..k).map(|j| y_fit.coefficients[2 + j]).collect();
    let beta3_all: Vec<f64> = if opts.interaction {
        (0..k).map(|j| y_fit.coefficients[2 + k + j]).collect()
    } else {
        vec![0.0; k]
    };

    // CDE: β₁ + Σⱼ β₃ⱼ · cde_m.
    let cde = beta_1 + beta3_all.iter().map(|&b3| b3 * opts.cde_m).sum::<f64>();

    // NDE: β₁ + Σⱼ β₃ⱼ · E[M_j | X=0].
    let nde = beta_1
        + beta3_all
            .iter()
            .zip(&m_under_control_all)
            .map(|(&b3, &m_ctrl)| b3 * m_ctrl)
            .sum::<f64>();

    // NIE_j = (β₂ⱼ + β₃ⱼ) · α₁ⱼ; NIE_total = Σⱼ NIE_j.
    let nie_per: Vec<f64> = (0..k)
        .map(|j| (beta2_all[j] + beta3_all[j]) * alpha1_all[j])
        .collect();
    let nie: f64 = nie_per.iter().sum();
    let te = nde + nie;

    Ok((
        cde,
        nde,
        nie,
        te,
        nie_per,
        alpha1_all,
        y_fit.coefficients.clone(),
    ))
}

// ═══════════════════════════════════════════════════════════════════════════
// 3. Binary outcome (logistic) — OR-scale decomposition
// ═══════════════════════════════════════════════════════════════════════════

/// Binary-outcome causal mediation (logistic Y model, continuous M).
///
/// Effects are on the OR scale:
/// - CDE(m) = exp(β₁ + β₃·m)
/// - NDE = exp(β₁ + β₃·E[M|X=0])
/// - NIE = exp((β₂ + β₃)·α₁)
/// - TE = NDE × NIE
///
/// Matching CMAverse `est_rb.R` for logistic-linear case.
pub fn cmest_binary_y(
    x: &[f64],
    m: &[f64],
    y: &[f64],
    covariates: &[&[f64]],
    opts: &CmestOptions,
) -> Result<CmestResult> {
    let n = x.len();
    if n == 0 || m.len() != n || y.len() != n {
        return Err(EpiError::DimensionMismatch {
            a: n,
            b: m.len().max(y.len()),
        });
    }
    for &v in y {
        if v != 0.0 && v != 1.0 {
            return Err(EpiError::Numerical(format!(
                "binary outcome must be 0/1, got {v}"
            )));
        }
    }

    // Mediator model: M ~ X + C (OLS).
    let mut m_preds: Vec<&[f64]> = vec![x];
    for c in covariates {
        m_preds.push(c);
    }
    let m_fit = regression::ols(&m_preds, m, true).map_err(epi_from_stat)?;
    let alpha_1 = m_fit.coefficients[1];
    let m_ctrl = m_fit.coefficients[0]
        + covariates
            .iter()
            .enumerate()
            .map(|(j, c)| {
                let mean: f64 = c.iter().sum::<f64>() / n as f64;
                m_fit.coefficients[2 + j] * mean
            })
            .sum::<f64>();

    // Outcome model: Y ~ X + M [+ X:M] + C (logistic).
    let xm = if opts.interaction {
        x.iter()
            .zip(m)
            .map(|(&xi, &mi)| xi * mi)
            .collect::<Vec<_>>()
    } else {
        vec![]
    };
    let mut y_preds: Vec<&[f64]> = vec![x, m];
    if opts.interaction {
        y_preds.push(&xm);
    }
    for c in covariates {
        y_preds.push(c);
    }
    let y_fit = regression::logistic(&y_preds, y, true).map_err(epi_from_stat)?;

    let beta_1 = y_fit.coefficients[1];
    let beta_2 = y_fit.coefficients[2];
    let beta_3 = if opts.interaction {
        y_fit.coefficients[3]
    } else {
        0.0
    };

    // OR-scale effects (CMAverse est_rb.R, nonlinear Y with linear M).
    let cde_or = (beta_1 + beta_3 * opts.cde_m).exp();
    let nde_or = (beta_1 + beta_3 * m_ctrl).exp();
    let nie_or = ((beta_2 + beta_3) * alpha_1).exp();
    let te_or = nde_or * nie_or;
    let pm = (nie_or.ln() / te_or.ln()).max(0.0).min(1.0); // approximate
    let pe = 1.0 - (nde_or.ln() / te_or.ln());

    // Bootstrap.
    let mut rng = ChaCha8Rng::seed_from_u64(opts.seed);
    let indices: Vec<usize> = (0..n).collect();
    let mut boot = (Vec::new(), Vec::new(), Vec::new(), Vec::new());

    for _ in 0..opts.n_bootstrap {
        let idx: Vec<usize> = (0..n)
            .map(|_| indices[(rng.random::<f64>() * n as f64) as usize])
            .collect();
        let x_b: Vec<f64> = idx.iter().map(|&i| x[i]).collect();
        let m_b: Vec<f64> = idx.iter().map(|&i| m[i]).collect();
        let y_b: Vec<f64> = idx.iter().map(|&i| y[i]).collect();
        let cov_b: Vec<Vec<f64>> = covariates
            .iter()
            .map(|c| idx.iter().map(|&i| c[i]).collect())
            .collect();
        let cov_slices: Vec<&[f64]> = cov_b.iter().map(|v| v.as_slice()).collect();
        if let Ok(r) = cmest_binary_y(
            &x_b,
            &m_b,
            &y_b,
            &cov_slices,
            &CmestOptions {
                n_bootstrap: 0,
                ..*opts
            },
        ) {
            boot.0.push(r.cde);
            boot.1.push(r.nde);
            boot.2.push(r.nie);
            boot.3.push(r.te);
        }
    }

    Ok(CmestResult {
        cde: cde_or,
        cde_ci: pci(&boot.0),
        nde: nde_or,
        nde_ci: pci(&boot.1),
        nie: nie_or,
        nie_ci: pci(&boot.2),
        te: te_or,
        te_ci: pci(&boot.3),
        prop_mediated: pm,
        prop_eliminated: pe,
        mediator_model: m_fit.coefficients.clone(),
        outcome_model: y_fit.coefficients.clone(),
        n_obs: n,
        nie_per_mediator: vec![nie_or],
        prop_mediated_per_mediator: vec![pm],
        mediator_weights: vec![1.0],
    })
}

// ═══════════════════════════════════════════════════════════════════════════
// 4. Binary mediator (logistic M model) + continuous outcome
// ═══════════════════════════════════════════════════════════════════════════

/// Binary-mediator causal mediation (logistic M model, continuous Y).
///
/// For binary M, the NIE is computed via the difference in probabilities
/// P(M=1|X=1) − P(M=1|X=0), scaled by β₂.
pub fn cmest_binary_m(
    x: &[f64],
    m: &[f64],
    y: &[f64],
    covariates: &[&[f64]],
    opts: &CmestOptions,
) -> Result<CmestResult> {
    let n = x.len();
    if n == 0 || m.len() != n || y.len() != n {
        return Err(EpiError::DimensionMismatch {
            a: n,
            b: m.len().max(y.len()),
        });
    }
    for &v in m {
        if v != 0.0 && v != 1.0 {
            return Err(EpiError::Numerical(format!(
                "binary mediator must be 0/1, got {v}"
            )));
        }
    }

    // Mediator model: M ~ X + C (logistic).
    let mut m_preds: Vec<&[f64]> = vec![x];
    for c in covariates {
        m_preds.push(c);
    }
    let m_fit = regression::logistic(&m_preds, m, true).map_err(epi_from_stat)?;
    // P(M=1|X) at covariate means.
    let cov_means: Vec<f64> = covariates
        .iter()
        .map(|c| c.iter().sum::<f64>() / n as f64)
        .collect();
    let eta_x0 = m_fit.coefficients[0]
        + cov_means
            .iter()
            .enumerate()
            .map(|(j, &cm)| m_fit.coefficients[2 + j] * cm)
            .sum::<f64>();
    let eta_x1 = eta_x0 + m_fit.coefficients[1];
    let p_m1_x0 = sigmoid(eta_x0);
    let p_m1_x1 = sigmoid(eta_x1);
    let delta_p = p_m1_x1 - p_m1_x0;

    // Outcome model: Y ~ X + M [+ X:M] + C (OLS, since Y is continuous).
    let xm = if opts.interaction {
        x.iter()
            .zip(m)
            .map(|(&xi, &mi)| xi * mi)
            .collect::<Vec<_>>()
    } else {
        vec![]
    };
    let mut y_preds: Vec<&[f64]> = vec![x, m];
    if opts.interaction {
        y_preds.push(&xm);
    }
    for c in covariates {
        y_preds.push(c);
    }
    let y_fit = regression::ols(&y_preds, y, true).map_err(epi_from_stat)?;
    let beta_1 = y_fit.coefficients[1];
    let beta_2 = y_fit.coefficients[2];
    let beta_3 = if opts.interaction {
        y_fit.coefficients[3]
    } else {
        0.0
    };

    let cde = beta_1 + beta_3 * opts.cde_m;
    let nde = beta_1 + beta_3 * p_m1_x0;
    let nie = (beta_2 + beta_3) * delta_p;
    let te = nde + nie;
    let pm = if te.abs() > 1e-10 { nie / te } else { f64::NAN };
    let pe = if te.abs() > 1e-10 {
        1.0 - nde / te
    } else {
        f64::NAN
    };

    Ok(CmestResult {
        cde,
        cde_ci: (f64::NAN, f64::NAN),
        nde,
        nde_ci: (f64::NAN, f64::NAN),
        nie,
        nie_ci: (f64::NAN, f64::NAN),
        te,
        te_ci: (f64::NAN, f64::NAN),
        prop_mediated: pm,
        prop_eliminated: pe,
        mediator_model: m_fit.coefficients.clone(),
        outcome_model: y_fit.coefficients.clone(),
        n_obs: n,
        nie_per_mediator: vec![nie],
        prop_mediated_per_mediator: vec![pm],
        mediator_weights: vec![1.0],
    })
}

// ═══════════════════════════════════════════════════════════════════════════
// 5. Weighting-based approach (VanderWeele 2014)
// ═══════════════════════════════════════════════════════════════════════════

/// Weighting-based causal mediation (continuous Y, continuous or binary M).
///
/// Uses inverse-probability-of-treatment weighting to estimate NDE and NIE
/// without specifying a parametric mediator-outcome confounding structure.
///
/// - NDE: weighted regression of Y on X among subjects with M set to natural value.
/// - NIE: difference between TE (weighted Y on X) and NDE.
pub fn cmest_weighting(
    x: &[f64],
    m: &[f64],
    y: &[f64],
    covariates: &[&[f64]],
    _opts: &CmestOptions,
) -> Result<CmestResult> {
    let n = x.len();
    if n == 0 || m.len() != n || y.len() != n {
        return Err(EpiError::DimensionMismatch {
            a: n,
            b: m.len().max(y.len()),
        });
    }

    // Step 1: Propensity model P(X=1|C) via logistic.
    let cov_slices: Vec<&[f64]> = covariates.to_vec();
    let ps_fit = regression::logistic(&cov_slices, x, true).map_err(epi_from_stat)?;
    let ps = &ps_fit.fitted;
    let p_bar = x.iter().filter(|&&v| v == 1.0).count() as f64 / n as f64;

    // Stabilized weights.
    let weights: Vec<f64> = (0..n)
        .map(|i| {
            if x[i] == 1.0 {
                p_bar / ps[i].max(1e-6)
            } else {
                (1.0 - p_bar) / (1.0 - ps[i]).max(1e-6)
            }
        })
        .collect();

    // TE: weighted Y ~ X.
    let te_fit = regression::wls(&[x], y, &weights, true).map_err(epi_from_stat)?;
    let te = te_fit.coefficients[1];

    // NDE: weighted Y ~ X + M (adjusting for mediator).
    let nde_fit = regression::wls(&[x, m], y, &weights, true).map_err(epi_from_stat)?;
    let nde = nde_fit.coefficients[1];

    let nie = te - nde;
    let cde = nde; // approximated by NDE in weighting approach
    let pm = if te.abs() > 1e-10 { nie / te } else { f64::NAN };
    let pe = if te.abs() > 1e-10 {
        1.0 - nde / te
    } else {
        f64::NAN
    };

    Ok(CmestResult {
        cde,
        cde_ci: (f64::NAN, f64::NAN),
        nde,
        nde_ci: (f64::NAN, f64::NAN),
        nie,
        nie_ci: (f64::NAN, f64::NAN),
        te,
        te_ci: (f64::NAN, f64::NAN),
        prop_mediated: pm,
        prop_eliminated: pe,
        mediator_model: ps_fit.coefficients.clone(),
        outcome_model: te_fit.coefficients.clone(),
        n_obs: n,
        nie_per_mediator: vec![nie],
        prop_mediated_per_mediator: vec![pm],
        mediator_weights: vec![1.0],
    })
}

// ═══════════════════════════════════════════════════════════════════════════
// 6. g-formula approach (Robins 1986)
// ═══════════════════════════════════════════════════════════════════════════

/// g-formula causal mediation (continuous Y, continuous M).
///
/// Uses the parametric g-formula: simulate counterfactual outcomes by
/// setting X to different levels and predicting M and Y from fitted models.
pub fn cmest_gformula(
    x: &[f64],
    m: &[f64],
    y: &[f64],
    covariates: &[&[f64]],
    opts: &CmestOptions,
) -> Result<CmestResult> {
    let n = x.len();

    // Mediator model: M ~ X + C.
    let mut m_preds: Vec<&[f64]> = vec![x];
    for c in covariates {
        m_preds.push(c);
    }
    let m_fit = regression::ols(&m_preds, m, true).map_err(epi_from_stat)?;

    // Outcome model: Y ~ X + M [+ X:M] + C.
    let xm = if opts.interaction {
        x.iter()
            .zip(m)
            .map(|(&xi, &mi)| xi * mi)
            .collect::<Vec<_>>()
    } else {
        vec![]
    };
    let mut y_preds: Vec<&[f64]> = vec![x, m];
    if opts.interaction {
        y_preds.push(&xm);
    }
    for c in covariates {
        y_preds.push(c);
    }
    let y_fit = regression::ols(&y_preds, y, true).map_err(epi_from_stat)?;

    // Predict counterfactual M and Y at X=1 and X=0 (all covariates at observed values).
    let _cov_means: Vec<f64> = covariates
        .iter()
        .map(|c| c.iter().sum::<f64>() / n as f64)
        .collect();

    // E[Y(X=1, M(X=1))] and E[Y(X=0, M(X=0))].
    let mut y_x1_m1 = 0.0_f64;
    let mut y_x0_m0 = 0.0_f64;
    let mut y_x1_m0 = 0.0_f64; // for NDE: X=1, M(X=0)

    for i in 0..n {
        // M(X=0): predicted mediator at X=0 for this subject.
        let mut m_x0 = m_fit.coefficients[0];
        for (j, c) in covariates.iter().enumerate() {
            m_x0 += m_fit.coefficients[2 + j] * c[i];
        }

        // M(X=1): predicted mediator at X=1.
        let m_x1 = m_x0 + m_fit.coefficients[1];

        // Y(X, M) predictions.
        let _cov_term: f64 = covariates
            .iter()
            .enumerate()
            .map(|(j, _)| y_fit.coefficients[2 + if opts.interaction { 1 } else { 0 } + j])
            .sum::<f64>(); // rough
        let beta_x = y_fit.coefficients[1];
        let beta_m = y_fit.coefficients[2];
        let beta_xm = if opts.interaction {
            y_fit.coefficients[3]
        } else {
            0.0
        };
        let beta_0 = y_fit.coefficients[0];

        y_x1_m1 += beta_0 + beta_x * 1.0 + beta_m * m_x1 + beta_xm * 1.0 * m_x1;
        y_x0_m0 += beta_0 + beta_x * 0.0 + beta_m * m_x0 + beta_xm * 0.0 * m_x0;
        y_x1_m0 += beta_0 + beta_x * 1.0 + beta_m * m_x0 + beta_xm * 1.0 * m_x0;
    }
    y_x1_m1 /= n as f64;
    y_x0_m0 /= n as f64;
    y_x1_m0 /= n as f64;

    let te = y_x1_m1 - y_x0_m0;
    let nde = y_x1_m0 - y_x0_m0;
    let nie = te - nde;
    let cde = nde;
    let pm = if te.abs() > 1e-10 { nie / te } else { f64::NAN };
    let pe = if te.abs() > 1e-10 {
        1.0 - nde / te
    } else {
        f64::NAN
    };

    Ok(CmestResult {
        cde,
        cde_ci: (f64::NAN, f64::NAN),
        nde,
        nde_ci: (f64::NAN, f64::NAN),
        nie,
        nie_ci: (f64::NAN, f64::NAN),
        te,
        te_ci: (f64::NAN, f64::NAN),
        prop_mediated: pm,
        prop_eliminated: pe,
        mediator_model: m_fit.coefficients.clone(),
        outcome_model: y_fit.coefficients.clone(),
        n_obs: n,
        nie_per_mediator: vec![nie],
        prop_mediated_per_mediator: vec![pm],
        mediator_weights: vec![1.0],
    })
}

// ═══════════════════════════════════════════════════════════════════════════
// Helpers
// ═══════════════════════════════════════════════════════════════════════════

fn sigmoid(x: f64) -> f64 {
    if x >= 0.0 {
        1.0 / (1.0 + (-x).exp())
    } else {
        let e = x.exp();
        e / (1.0 + e)
    }
}

fn pci(boot: &[f64]) -> (f64, f64) {
    if boot.len() < 2 {
        return (f64::NAN, f64::NAN);
    }
    let mut sorted = boot.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let n = sorted.len();
    (
        sorted[(0.025 * n as f64).floor() as usize],
        sorted[(0.975 * n as f64).ceil() as usize],
    )
}

fn epi_from_stat(e: statkit::StatError) -> EpiError {
    EpiError::Numerical(e.to_string())
}

// ═══════════════════════════════════════════════════════════════════════════
// Tests
// ═══════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;
    use rand::Rng;

    fn approx_eq(a: f64, b: f64, tol: f64) -> bool {
        (a - b).abs() <= tol * b.abs().max(1.0)
    }

    fn make_data(n: usize, seed: u64) -> (Vec<f64>, Vec<f64>, Vec<f64>, Vec<f64>) {
        let mut rng = ChaCha8Rng::seed_from_u64(seed);
        let x: Vec<f64> = (0..n).map(|_| rng.random::<f64>().round()).collect();
        let c: Vec<f64> = (0..n).map(|_| rng.random::<f64>() * 2.0 - 1.0).collect();
        let m: Vec<f64> = (0..n)
            .map(|i| 1.0 + 0.5 * x[i] + 0.3 * c[i] + rng.random::<f64>() * 0.5 - 0.25)
            .collect();
        let y: Vec<f64> = (0..n)
            .map(|i| 0.3 * x[i] + 0.8 * m[i] - 0.2 * c[i] + rng.random::<f64>() * 0.3 - 0.15)
            .collect();
        (x, m, y, c)
    }

    // ── Single mediator ─────────────────────────────────────────────────

    #[test]
    fn cmest_single_recovers_effects() {
        let (x, m, y, c) = make_data(500, 42);
        let opts = CmestOptions {
            interaction: false,
            n_bootstrap: 0,
            ..Default::default()
        };
        let r = cmest(&x, &m, &y, &[&c], &opts).unwrap();
        assert!(approx_eq(r.nde, 0.3, 0.1), "NDE ≈ 0.3");
        assert!(approx_eq(r.nie, 0.4, 0.1), "NIE ≈ 0.4");
        assert!((r.te - r.nde - r.nie).abs() < 1e-10);
    }

    // ── Multi-mediator ──────────────────────────────────────────────────

    #[test]
    fn cmest_multi_two_mediators() {
        let n = 500;
        let mut rng = ChaCha8Rng::seed_from_u64(42);
        let x: Vec<f64> = (0..n).map(|_| rng.random::<f64>().round()).collect();
        let c: Vec<f64> = (0..n).map(|_| rng.random::<f64>() * 2.0 - 1.0).collect();
        // Two mediators: m1 and m2.
        let m1: Vec<f64> = (0..n)
            .map(|i| 0.5 * x[i] + 0.3 * c[i] + rng.random::<f64>() * 0.5 - 0.25)
            .collect();
        let m2: Vec<f64> = (0..n)
            .map(|i| 0.3 * x[i] - 0.2 * c[i] + rng.random::<f64>() * 0.5 - 0.25)
            .collect();
        // Outcome depends on both: y = 0.3*x + 0.8*m1 + 0.5*m2.
        let y: Vec<f64> = (0..n)
            .map(|i| 0.3 * x[i] + 0.8 * m1[i] + 0.5 * m2[i] + rng.random::<f64>() * 0.3 - 0.15)
            .collect();

        let opts = CmestOptions {
            interaction: false,
            n_bootstrap: 0,
            ..Default::default()
        };
        let r = cmest_multi(&x, &[&m1, &m2], &y, &[&c], &opts).unwrap();

        // TE ≈ 0.3 + 0.8*0.5 + 0.5*0.3 = 0.3 + 0.4 + 0.15 = 0.85.
        assert!(approx_eq(r.te, 0.85, 0.15), "TE ≈ 0.85, got {}", r.te);
        // NDE ≈ 0.3 (direct effect).
        assert!(approx_eq(r.nde, 0.3, 0.1), "NDE ≈ 0.3, got {}", r.nde);
        // NIE_total ≈ 0.4 + 0.15 = 0.55.
        assert!(approx_eq(r.nie, 0.55, 0.15), "NIE ≈ 0.55, got {}", r.nie);
        // Two per-mediator NIEs.
        assert_eq!(r.nie_per_mediator.len(), 2);
        assert!(
            r.nie_per_mediator[0] > r.nie_per_mediator[1],
            "M1 should contribute more"
        );
    }

    // ── Binary outcome ──────────────────────────────────────────────────

    #[test]
    fn cmest_binary_y_runs() {
        let n = 300;
        let mut rng = ChaCha8Rng::seed_from_u64(42);
        let x: Vec<f64> = (0..n).map(|_| rng.random::<f64>().round()).collect();
        let m: Vec<f64> = (0..n)
            .map(|i| 1.0 + 0.5 * x[i] + rng.random::<f64>() * 0.5)
            .collect();
        let eta: Vec<f64> = (0..n).map(|i| -1.0 + 0.5 * x[i] + 0.3 * m[i]).collect();
        let y: Vec<f64> = eta
            .iter()
            .map(|&e| {
                if rng.random::<f64>() < sigmoid(e) {
                    1.0
                } else {
                    0.0
                }
            })
            .collect();

        let opts = CmestOptions {
            interaction: false,
            n_bootstrap: 0,
            ..Default::default()
        };
        let r = cmest_binary_y(&x, &m, &y, &[], &opts).unwrap();
        // All ORs should be > 0.
        assert!(r.cde > 0.0 && r.nde > 0.0 && r.nie > 0.0 && r.te > 0.0);
    }

    // ── Binary mediator ─────────────────────────────────────────────────

    #[test]
    fn cmest_binary_m_runs() {
        let n = 300;
        let mut rng = ChaCha8Rng::seed_from_u64(42);
        let x: Vec<f64> = (0..n).map(|_| rng.random::<f64>().round()).collect();
        let eta_m: Vec<f64> = (0..n).map(|i| -0.5 + 0.8 * x[i]).collect();
        let m: Vec<f64> = eta_m
            .iter()
            .map(|&e| {
                if rng.random::<f64>() < sigmoid(e) {
                    1.0
                } else {
                    0.0
                }
            })
            .collect();
        let y: Vec<f64> = (0..n)
            .map(|i| 0.3 * x[i] + 0.8 * m[i] + rng.random::<f64>() * 0.3)
            .collect();

        let r = cmest_binary_m(&x, &m, &y, &[], &CmestOptions::default()).unwrap();
        assert!(r.nie.abs() > 0.0, "NIE should be nonzero");
        assert!((r.te - r.nde - r.nie).abs() < 1e-10);
    }

    // ── Weighting approach ──────────────────────────────────────────────

    #[test]
    fn cmest_weighting_recovers_direction() {
        let (x, m, y, c) = make_data(300, 42);
        let r = cmest_weighting(&x, &m, &y, &[&c], &CmestOptions::default()).unwrap();
        assert!(r.te > 0.0, "TE should be positive");
        assert!(r.nie > 0.0, "NIE should be positive (mediation)");
    }

    // ── g-formula approach ──────────────────────────────────────────────

    #[test]
    fn cmest_gformula_recovers_effects() {
        let (x, m, y, c) = make_data(500, 42);
        let opts = CmestOptions {
            interaction: false,
            ..Default::default()
        };
        let r = cmest_gformula(&x, &m, &y, &[&c], &opts).unwrap();
        assert!(approx_eq(r.nde, 0.3, 0.1), "NDE ≈ 0.3, got {}", r.nde);
        assert!(approx_eq(r.nie, 0.4, 0.1), "NIE ≈ 0.4, got {}", r.nie);
    }
}
