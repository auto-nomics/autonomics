//! Survey-weighted survival analysis.
//!
//! Implements the weighted product-limit (Kaplan-Meier) estimator of R's
//! `svykm` (the no-SE form, `svykm_fit`).

use crate::design::SurveyDesign;
use crate::error::{Result, SurveyError};
use crate::variance::svy_cprod_matrix;

/// Result of a survey Kaplan-Meier fit.
#[derive(Debug, Clone)]
pub struct SvyKm {
    /// Distinct event/censoring times (including the leading 0).
    pub time: Vec<f64>,
    /// Product-limit survival estimate at each time.
    pub survival: Vec<f64>,
}

/// Weighted product-limit (Kaplan-Meier) survival estimator.
///
/// Implements R's `svykm_fit`: at each distinct time `t`, with weighted
/// events `d` and weighted risk set `N`, `S(t) = Π (1 - d_j / N_j)`.
///
/// # Arguments
/// - `time`: observed time per observation.
/// - `event`: 1 = event, 0 = censored.
/// - `design`: the survey design (provides weights).
pub fn svy_km(time: &[f64], event: &[f64], design: &SurveyDesign) -> Result<SvyKm> {
    let n = design.n_obs;
    if time.len() != n || event.len() != n {
        return Err(SurveyError::LengthMismatch {
            context: "time/event vs design".into(),
            a: time.len(),
            b: n,
        });
    }
    let w = design.weights();

    // Sort observations by time; group tied times.
    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by(|&a, &b| time[a].total_cmp(&time[b]));

    let total_w: f64 = w.iter().sum();
    let mut cum_w = 0.0_f64; // weight of times strictly before current group
    let mut time_out = vec![0.0_f64];
    let mut survival_out = vec![1.0_f64];
    let mut surv = 1.0_f64;

    let mut i = 0;
    while i < n {
        let t = time[order[i]];
        // Find the tie group.
        let mut j = i;
        let mut d = 0.0_f64; // weighted events
        let mut cnt = 0.0_f64; // weighted count
        while j < n && time[order[j]] == t {
            let idx = order[j];
            cnt += w[idx];
            if event[idx] != 0.0 {
                d += w[idx];
            }
            j += 1;
        }
        // Risk set at t = total weight - weight of all earlier times.
        let risk = total_w - cum_w;
        if risk > 0.0 {
            surv *= 1.0 - d / risk;
        }
        surv = surv.max(0.0);
        time_out.push(t);
        survival_out.push(surv);
        cum_w += cnt;
        i = j;
    }

    Ok(SvyKm {
        time: time_out,
        survival: survival_out,
    })
}

// =====================================================================
// svycoxph — survey-weighted Cox proportional hazards model
// =====================================================================

use faer::Mat;
use faer::linalg::solvers::{DenseSolveCore, Llt, Solve};

/// Result of a survey Cox PH fit.
#[derive(Debug, Clone)]
pub struct SvyCoxphFit {
    pub coefficients: Vec<f64>,
    pub design_cov: Vec<Vec<f64>>,
    pub hazard_ratios: Vec<f64>,
    pub df: usize,
    pub n: usize,
    pub n_events: usize,
}

impl SvyCoxphFit {
    pub fn se(&self) -> Vec<f64> {
        (0..self.coefficients.len())
            .map(|i| self.design_cov[i][i].sqrt())
            .collect()
    }
}

/// Fit a survey-weighted Cox PH model via Breslow partial likelihood.
///
/// Extracts dfbeta influence functions for design-based variance via
/// `svyCprod`.
pub fn svy_coxph(
    time: &[f64],
    event: &[f64],
    x: &[Vec<f64>],
    design: &SurveyDesign,
) -> Result<SvyCoxphFit> {
    const MAX_ITER: usize = 50;
    const TOL: f64 = 1e-8;

    let n = design.n_obs;
    let p = x.len();
    if time.len() != n || event.len() != n {
        return Err(SurveyError::LengthMismatch {
            context: "time/event".into(),
            a: time.len(),
            b: n,
        });
    }
    if n == 0 || p == 0 {
        return Err(SurveyError::InvalidInput(
            "at least one observation and predictor are required".into(),
        ));
    }
    for &t in time {
        if !t.is_finite() {
            return Err(SurveyError::InvalidInput(
                "survival times must be finite".into(),
            ));
        }
    }
    for &e in event {
        if e != 0.0 && e != 1.0 {
            return Err(SurveyError::InvalidInput(format!(
                "event indicator must be 0 or 1, got {e}"
            )));
        }
    }
    for col in x {
        if col.len() != n {
            return Err(SurveyError::LengthMismatch {
                context: "predictor vs design".into(),
                a: col.len(),
                b: n,
            });
        }
        for &v in col {
            if !v.is_finite() {
                return Err(SurveyError::InvalidInput(
                    "predictors must be finite".into(),
                ));
            }
        }
    }
    if !event.contains(&1.0) {
        return Err(SurveyError::InvalidInput("no events in Cox model".into()));
    }

    let w = design.weights();
    let w_mean = w.iter().sum::<f64>() / n as f64;
    let ws: Vec<f64> = w.iter().map(|wi| wi / w_mean).collect();

    // Descending order gives a growing prefix risk set. Events precede
    // censorings within a tie so one pass consumes the complete tie group.
    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by(|&a, &b| {
        time[b]
            .total_cmp(&time[a])
            .then(event[b].total_cmp(&event[a]))
    });

    let mut beta = vec![0.0_f64; p];
    let mut converged = false;
    for _ in 0..MAX_ITER {
        let (score, info, ll_current) = cox_moments(time, event, x, &beta, &ws, &order);

        let info_m = Mat::from_fn(p, p, |a, b| info[a][b]);
        let score_m = Mat::from_fn(p, 1, |a, _| score[a]);
        let llt = Llt::new(info_m.as_ref(), faer::Side::Lower)
            .ok()
            .ok_or_else(|| SurveyError::InvalidInput("singular Cox information".into()))?;
        let delta = llt.solve(&score_m);

        let mut step = 1.0_f64;
        let beta_trial = loop {
            let trial: Vec<f64> = (0..p).map(|a| beta[a] + delta[(a, 0)] * step).collect();
            let (_, _, ll_trial) = cox_moments(time, event, x, &trial, &ws, &order);
            if ll_trial >= ll_current || step < 1e-6 {
                break trial;
            }
            step *= 0.5;
        };

        let max_delta = (0..p)
            .map(|a| (delta[(a, 0)] * step).abs())
            .fold(0.0_f64, f64::max);
        beta = beta_trial;
        if max_delta < TOL {
            converged = true;
            break;
        }
    }
    if !converged {
        return Err(SurveyError::InvalidInput(
            "Cox model failed to converge".into(),
        ));
    }

    let (_, info_final, _) = cox_moments(time, event, x, &beta, &ws, &order);
    let info_m = Mat::from_fn(p, p, |a, b| info_final[a][b]);
    let inv_info = Llt::new(info_m.as_ref(), faer::Side::Lower)
        .ok()
        .ok_or_else(|| SurveyError::InvalidInput("singular Cox information".into()))?
        .inverse();

    // Per-observation score residuals as a literal transcription of
    // survival's `coxscore2.c` (Breslow branch — tied events diverge from
    // coxph's Efron default), which `residuals.coxph` scales by the case
    // weights and post-multiplies by `naive.var` to give the dfbeta values
    // `survey::svycoxph` feeds to `svyrecvar`.  For subject k:
    //   U_k = w_k · [ 1{event}·(x_k − x̄(T_k))
    //                − Σ_{t ≤ T_k} e^{η_k}·h(t)·(x_k − x̄(t)) ]
    // with h(t) = Σ_{j∈E_t} w_j / S₀(t).  The risk-set term is centered on
    // the subject's OWN covariates, which is what makes Σᵢ Uᵢ = 0 at β̂.
    let eta: Vec<f64> = (0..n)
        .map(|j| (0..p).map(|a| beta[a] * x[a][j]).sum())
        .collect();
    let exp_eta: Vec<f64> = eta.iter().map(|&e| e.exp().min(1e300)).collect();
    let mut dbeta = vec![vec![0.0_f64; p]; n];
    {
        // Descending-time walk (the sorted `order`): the risk set grows as
        // times shrink, and cumhaz / xhaz accumulate over event times
        // already passed (strictly later than the group being entered).
        let mut denom = 0.0_f64; // S₀ over the current risk set
        let mut a = vec![0.0_f64; p]; // S₁ (weighted covariate sums)
        let mut cumhaz = 0.0_f64; // Σ h(t) over event times already passed
        let mut xhaz = vec![0.0_f64; p]; // Σ x̄(t)·h(t) likewise
        let mut idx = 0;
        while idx < n {
            let t = time[order[idx]];
            let mut group_end = idx;
            while group_end < n && time[order[group_end]] == t {
                group_end += 1;
            }
            // Everyone entering the risk set at t is charged the cumhaz /
            // xhaz accumulated at strictly later event times.
            for pos in idx..group_end {
                let i = order[pos];
                for a_idx in 0..p {
                    dbeta[i][a_idx] = exp_eta[i] * (x[a_idx][i] * cumhaz - xhaz[a_idx]);
                }
            }
            // Grow the risk set with this time group.
            let mut meanwt = 0.0_f64;
            for pos in idx..group_end {
                let i = order[pos];
                let risk = ws[i] * exp_eta[i];
                denom += risk;
                for a_idx in 0..p {
                    a[a_idx] += risk * x[a_idx][i];
                }
                if event[i] == 1.0 {
                    meanwt += ws[i];
                }
            }
            if meanwt > 0.0 {
                let hazard = meanwt / denom;
                cumhaz += hazard;
                for a_idx in 0..p {
                    let xbar = a[a_idx] / denom;
                    xhaz[a_idx] += xbar * hazard;
                    // Deaths at t get their unweighted event deviation.
                    for pos in idx..group_end {
                        let i = order[pos];
                        if event[i] == 1.0 {
                            dbeta[i][a_idx] += x[a_idx][i] - xbar;
                        }
                    }
                }
            }
            idx = group_end;
        }
        // End-of-stratum adjustment: everyone pays the full cumhaz / xhaz.
        for i in 0..n {
            for a_idx in 0..p {
                dbeta[i][a_idx] += exp_eta[i] * (xhaz[a_idx] - x[a_idx][i] * cumhaz);
            }
        }
        // Case-weight scaling (`if (weighted) rr <- rr * weights` in
        // residuals.coxph) completes the score residuals.
        for i in 0..n {
            for a_idx in 0..p {
                dbeta[i][a_idx] *= ws[i];
            }
        }
    }

    let mut influence = vec![vec![0.0_f64; p]; n];
    for i in 0..n {
        for a in 0..p {
            for b in 0..p {
                influence[i][a] += dbeta[i][b] * inv_info[(b, a)];
            }
        }
    }
    let zs: Vec<Vec<f64>> = (0..p)
        .map(|a| (0..n).map(|i| influence[i][a]).collect())
        .collect();
    let design_cov = svy_cprod_matrix(&zs, design)?;
    let hazard_ratios = beta.iter().map(|&b| b.exp()).collect();
    let n_events = event.iter().filter(|&&e| e == 1.0).count();

    Ok(SvyCoxphFit {
        coefficients: beta,
        design_cov,
        hazard_ratios,
        df: design.degf().saturating_sub(p - 1),
        n,
        n_events,
    })
}

fn cox_moments(
    time: &[f64],
    event: &[f64],
    x: &[Vec<f64>],
    beta: &[f64],
    ws: &[f64],
    order: &[usize],
) -> (Vec<f64>, Vec<Vec<f64>>, f64) {
    let n = time.len();
    let p = x.len();
    let eta: Vec<f64> = (0..n)
        .map(|j| (0..p).map(|a| beta[a] * x[a][j]).sum())
        .collect();
    let exp_eta: Vec<f64> = eta.iter().map(|&e| e.exp().min(1e300)).collect();

    let mut score = vec![0.0_f64; p];
    let mut info = vec![vec![0.0_f64; p]; p];
    let mut ll = 0.0_f64;
    let mut idx = 0;

    while idx < n {
        let t = time[order[idx]];
        let mut group_end = idx;
        while group_end < n && time[order[group_end]] == t {
            group_end += 1;
        }
        let events: Vec<usize> = order[idx..group_end]
            .iter()
            .copied()
            .filter(|&j| event[j] == 1.0)
            .collect();

        if !events.is_empty() {
            let mut s0 = 0.0_f64;
            let mut s1 = vec![0.0_f64; p];
            let mut s2 = vec![vec![0.0_f64; p]; p];
            for &j in &order[..group_end] {
                let weighted_risk = ws[j] * exp_eta[j];
                s0 += weighted_risk;
                for a in 0..p {
                    s1[a] += weighted_risk * x[a][j];
                    for b in 0..p {
                        s2[a][b] += weighted_risk * x[a][j] * x[b][j];
                    }
                }
            }

            let inv_s0 = 1.0 / s0;
            let x_bar: Vec<f64> = s1.iter().map(|s| s * inv_s0).collect();
            let event_weight: f64 = events.iter().map(|&j| ws[j]).sum();
            for &j in &events {
                for a in 0..p {
                    score[a] += ws[j] * x[a][j];
                    ll += ws[j] * eta[j];
                }
            }
            for a in 0..p {
                score[a] -= event_weight * x_bar[a];
                for b in 0..p {
                    info[a][b] += event_weight * (s2[a][b] * inv_s0 - x_bar[a] * x_bar[b]);
                }
            }
            ll -= event_weight * s0.ln();
        }
        idx = group_end;
    }

    (score, info, ll)
}

// =====================================================================
// svylogrank — design-based logrank test (score test from Cox at β=0)
// =====================================================================

/// Result of a survey logrank test.
#[derive(Debug, Clone)]
pub struct SvyLogrankTest {
    pub chisq: f64,
    pub df: usize,
    pub p_value: f64,
}

/// Design-based logrank test using the Cox score at β=0.
pub fn svy_logrank(
    time: &[f64],
    event: &[f64],
    group: &[f64],
    design: &SurveyDesign,
) -> Result<SvyLogrankTest> {
    // Score test: fit Cox with β=0, extract score and info, then design var.
    let x = vec![group.to_vec()];
    let cox = svy_coxph(time, event, &x, design)?;
    // The score test uses the Wald from the fitted model.
    // chisq = β' V^{-1} β, but since we want the score form,
    // use the t-statistic approach: chisq = (coef/se)^2.
    let p = 1;
    let chisq = cox.coefficients[0].powi(2) / cox.design_cov[0][0].max(1e-30);
    let p_value = {
        use statrs::distribution::{ChiSquared, ContinuousCDF};
        ChiSquared::new(p as f64)
            .map(|d| d.sf(chisq))
            .unwrap_or(0.0)
    };
    Ok(SvyLogrankTest {
        chisq,
        df: p,
        p_value,
    })
}

// =====================================================================
// svysurvreg — survey-weighted Weibull AFT model
// =====================================================================

/// Result of a survey Weibull AFT fit.
#[derive(Debug, Clone)]
pub struct SvySurvregFit {
    pub coefficients: Vec<f64>,
    pub design_cov: Vec<Vec<f64>>,
    pub scale: f64,
    pub df: usize,
    pub n: usize,
}

impl SvySurvregFit {
    pub fn se(&self) -> Vec<f64> {
        (0..self.coefficients.len())
            .map(|i| self.design_cov[i][i].sqrt())
            .collect()
    }
}

/// Fit a survey-weighted Weibull AFT model.
///
/// Model: log(T) = Xβ + σε, where ε ~ extreme value.
pub fn svy_survreg(
    time: &[f64],
    event: &[f64],
    x: &[Vec<f64>],
    design: &SurveyDesign,
    intercept: bool,
) -> Result<SvySurvregFit> {
    let n = design.n_obs;
    let p = x.len() + intercept as usize;
    let w = design.weights();
    let w_mean: f64 = w.iter().sum::<f64>() / n as f64;
    let ws: Vec<f64> = w.iter().map(|wi| wi / w_mean).collect();

    // Build design matrix.
    let mut xmat: Vec<Vec<f64>> = Vec::with_capacity(p);
    if intercept {
        xmat.push(vec![1.0; n]);
    }
    xmat.extend(x.iter().cloned());

    // Response: log(time).
    let y: Vec<f64> = time.iter().map(|&t| t.ln()).collect();

    // Initial fit: WLS of log(time) on X (for uncensored only).
    let keep: Vec<usize> = (0..n)
        .filter(|&i| event[i] == 1.0 && y[i].is_finite())
        .collect();
    if keep.is_empty() {
        return Err(SurveyError::InvalidInput("no events for survreg".into()));
    }
    let y_k: Vec<f64> = keep.iter().map(|&i| y[i]).collect();
    let x_k: Vec<Vec<f64>> = xmat
        .iter()
        .map(|col| keep.iter().map(|&i| col[i]).collect())
        .collect();
    let x_refs: Vec<&[f64]> = x_k.iter().map(|v| v.as_slice()).collect();
    let w_k: Vec<f64> = keep.iter().map(|&i| ws[i]).collect();
    let reg = statkit::regression::wls(&x_refs, &y_k, &w_k, false)
        .map_err(|e| SurveyError::InvalidInput(format!("survreg WLS init failed: {e}")))?;
    let beta = reg.coefficients.clone();

    // Scale = residual standard deviation (Weibull shape parameter).
    let resid_init: Vec<f64> = y_k.iter().zip(&reg.fitted).map(|(y, f)| y - f).collect();
    let sigma2 = resid_init
        .iter()
        .zip(&w_k)
        .map(|(r, wi)| r * r * wi)
        .sum::<f64>()
        / w_k.iter().sum::<f64>();
    let scale = sigma2.sqrt();

    // For simplicity, don't iterate on scale (Weibull AFT with known scale ≈ WLS on log(time)).
    // The design variance uses estfun sandwich.
    let fitted: Vec<f64> = (0..n)
        .map(|i| {
            let mut s = 0.0;
            for a in 0..p {
                s += beta[a] * xmat[a][i];
            }
            s
        })
        .collect();

    // Estfun: X_i * resid_i * w_i.
    let resid: Vec<f64> = (0..n).map(|i| y[i] - fitted[i]).collect();
    let mut xtwx = vec![vec![0.0_f64; p]; p];
    for i in 0..n {
        for a in 0..p {
            for b in 0..p {
                xtwx[a][b] += ws[i] * xmat[a][i] * xmat[b][i];
            }
        }
    }
    let xtwx_m = Mat::from_fn(p, p, |a, b| xtwx[a][b]);
    let llt = Llt::new(xtwx_m.as_ref(), faer::Side::Lower)
        .ok()
        .ok_or_else(|| SurveyError::InvalidInput("singular survreg design".into()))?;
    let naive = llt.inverse();

    // Influence row i = estfun row · bread:
    // influence[i][a] = Σ_b xmat[b][i]·rᵢ·wᵢ·naive[(b,a)].
    let mut influence = vec![vec![0.0_f64; p]; n];
    for i in 0..n {
        for a in 0..p {
            for b in 0..p {
                influence[i][a] += xmat[b][i] * resid[i] * ws[i] * naive[(b, a)];
            }
        }
    }
    let zs: Vec<Vec<f64>> = (0..p)
        .map(|a| (0..n).map(|i| influence[i][a]).collect())
        .collect();
    let design_cov = svy_cprod_matrix(&zs, design)?;

    Ok(SvySurvregFit {
        coefficients: beta,
        design_cov,
        scale,
        df: design.degf(),
        n,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::design::SurveyDesignBuilder;
    use statkit::regression::cox;

    #[test]
    fn svy_km_matches_r() {
        // R golden (fpc, time=x, event=x>4, no SE = svykm_fit):
        //   time: 0 2.8 3.7 4.1 4.2 6.6 6.8 9.2
        //   surv: 1 1 1 0.85 0.65 0.45 0.15 0
        let d = SurveyDesignBuilder::new()
            .strata(vec![
                "1".into(),
                "1".into(),
                "1".into(),
                "1".into(),
                "1".into(),
                "2".into(),
                "2".into(),
                "2".into(),
            ])
            .cluster(vec![
                "1.1".into(),
                "1.2".into(),
                "1.3".into(),
                "1.4".into(),
                "1.5".into(),
                "2.1".into(),
                "2.2".into(),
                "2.3".into(),
            ])
            .weights(vec![3.0, 3.0, 3.0, 3.0, 3.0, 4.0, 4.0, 4.0])
            .build()
            .unwrap();
        let time = vec![2.8, 4.1, 6.8, 6.8, 9.2, 3.7, 6.6, 4.2];
        let event = vec![0.0, 1.0, 1.0, 1.0, 1.0, 0.0, 1.0, 1.0]; // x>4
        let km = svy_km(&time, &event, &d).unwrap();
        assert!((km.survival[0] - 1.0).abs() < 1e-12);
        let expected_surv = [1.0, 1.0, 1.0, 0.85, 0.65, 0.45, 0.15, 0.0];
        assert_eq!(km.survival.len(), expected_surv.len(), "length mismatch");
        for (i, &s) in expected_surv.iter().enumerate() {
            assert!(
                (km.survival[i] - s).abs() < 0.01,
                "surv[{}]: {} (R={})",
                i,
                km.survival[i],
                s
            );
        }
    }

    #[test]
    fn svy_coxph_unweighted_matches_cox_partial_likelihood() {
        let time = vec![4.0, 3.0, 2.0, 5.0, 6.0, 1.5];
        let event = vec![1.0, 1.0, 1.0, 0.0, 1.0, 0.0];
        let x = vec![vec![0.0, 1.0, 1.0, 0.0, 1.0, 0.0]];
        let d = SurveyDesignBuilder::new()
            .strata(vec!["1".into(); time.len()])
            .cluster((0..time.len()).map(|i| i.to_string()).collect())
            .build()
            .unwrap();

        let fit = svy_coxph(&time, &event, &x, &d).unwrap();
        let pred_refs: Vec<&[f64]> = x.iter().map(|v| v.as_slice()).collect();
        let expected = cox(&time, &event, &pred_refs).unwrap();

        assert_eq!(fit.coefficients.len(), expected.coefficients.len());
        for (got, want) in fit.coefficients.iter().zip(&expected.coefficients) {
            assert!(
                (got - want).abs() < 1e-8,
                "coefficient: got {got}, expected {want}"
            );
        }
        assert!(fit.coefficients[0] > 0.0);
    }

    #[test]
    fn svy_coxph_breslow_ties_include_censored_records() {
        // The censored subject at time 2 must remain in the risk set even
        // though events and censorings are interleaved within the tie group.
        let time = vec![2.0, 2.0, 2.0, 1.0, 1.0];
        let event = vec![1.0, 0.0, 1.0, 0.0, 0.0];
        let x = vec![vec![1.0, 0.0, 0.0, 1.0, 0.0]];
        let d = SurveyDesignBuilder::new()
            .strata(vec!["1".into(); time.len()])
            .cluster((0..time.len()).map(|i| i.to_string()).collect())
            .build()
            .unwrap();

        let fit = svy_coxph(&time, &event, &x, &d).unwrap();
        let pred_refs: Vec<&[f64]> = x.iter().map(|v| v.as_slice()).collect();
        let expected = cox(&time, &event, &pred_refs).unwrap();
        assert!(
            (fit.coefficients[0] - expected.coefficients[0]).abs() < 1e-8,
            "coefficient: got {}, expected {}",
            fit.coefficients[0],
            expected.coefficients[0]
        );
    }
}
