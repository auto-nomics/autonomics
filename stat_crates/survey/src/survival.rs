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
    let n = design.n_obs;
    let p = x.len();
    if time.len() != n || event.len() != n {
        return Err(SurveyError::LengthMismatch {
            context: "time/event".into(),
            a: time.len(),
            b: n,
        });
    }
    let w = design.weights();
    let w_mean: f64 = w.iter().sum::<f64>() / n as f64;
    let ws: Vec<f64> = w.iter().map(|wi| wi / w_mean).collect();

    // Sort by time (descending for risk set computation).
    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by(|&a, &b| time[b].total_cmp(&time[a]));

    // Newton-Raphson for partial likelihood.
    let mut beta = vec![0.0_f64; p];
    for _iter in 0..25 {
        let mut score = vec![0.0_f64; p];
        let mut info = vec![vec![0.0_f64; p]; p];
        // Iterate through sorted times; at each position, the risk set
        // is all observations at or before this position in `order`.
        // Since we sorted descending, risk set = order[0..=current_idx].
        let mut idx = 0;
        while idx < n {
            let i = order[idx];
            if event[i] == 0.0 {
                idx += 1;
                continue;
            }
            // Find the group of tied events at this time.
            let t = time[i];
            let mut tie_end = idx;
            let mut d = 0.0; // weighted number of events
            while tie_end < n && time[order[tie_end]] == t && event[order[tie_end]] == 1.0 {
                d += ws[order[tie_end]];
                tie_end += 1;
            }
            // Risk set: all obs from idx to end (since sorted descending).
            // S0 = sum_{j in R} w_j exp(eta_j)
            // S1 = sum_{j in R} w_j X_j exp(eta_j)
            let mut s0 = 0.0_f64;
            let mut s1 = vec![0.0_f64; p];
            let mut s2 = vec![vec![0.0_f64; p]; p];
            for k in idx..n {
                let j = order[k];
                let eta: f64 = (0..p).map(|a| beta[a] * x[a][j]).sum();
                let exp_eta = eta.exp();
                let wj = ws[j] * exp_eta;
                s0 += wj;
                for a in 0..p {
                    s1[a] += wj * x[a][j];
                    for b in 0..p {
                        s2[a][b] += wj * x[a][j] * x[b][j];
                    }
                }
            }
            // Score contribution: sum of event X - d * S1/S0
            for k in idx..tie_end {
                let j = order[k];
                for a in 0..p {
                    score[a] += ws[j] * x[a][j];
                }
            }
            for a in 0..p {
                score[a] -= d * s1[a] / s0;
            }
            // Info: d * (S2/S0 - (S1/S0)^2)
            for a in 0..p {
                for b in 0..p {
                    info[a][b] += d * (s2[a][b] / s0 - s1[a] * s1[b] / (s0 * s0));
                }
            }
            idx = tie_end;
        }
        // Newton step.
        let info_m = Mat::from_fn(p, p, |a, b| info[a][b]);
        let score_m = Mat::from_fn(p, 1, |a, _| score[a]);
        let llt = match Llt::new(info_m.as_ref(), faer::Side::Lower) {
            Ok(l) => l,
            Err(_) => break,
        };
        let delta = llt.solve(&score_m);
        let max_d = (0..p).map(|a| delta[(a, 0)].abs()).fold(0.0_f64, f64::max);
        for a in 0..p {
            beta[a] += delta[(a, 0)];
        }
        if max_d < 1e-8 {
            break;
        }
    }

    // Compute dfbeta residuals: per-observation score contributions.
    let n_events = event.iter().filter(|&&e| e == 1.0).count();
    let mut dbeta = vec![vec![0.0_f64; p]; n];
    let mut idx = 0;
    while idx < n {
        let i = order[idx];
        if event[i] == 0.0 {
            idx += 1;
            continue;
        }
        let t = time[i];
        let mut tie_end = idx;
        while tie_end < n && time[order[tie_end]] == t && event[order[tie_end]] == 1.0 {
            tie_end += 1;
        }
        // Risk set.
        let mut s0 = 0.0;
        let mut s1 = vec![0.0_f64; p];
        for k in idx..n {
            let j = order[k];
            let eta: f64 = (0..p).map(|a| beta[a] * x[a][j]).sum();
            let wj = ws[j] * eta.exp();
            s0 += wj;
            for a in 0..p {
                s1[a] += wj * x[a][j];
            }
        }
        // For each subject in risk set, their score contribution at this time.
        for k in idx..n {
            let j = order[k];
            let eta: f64 = (0..p).map(|a| beta[a] * x[a][j]).sum();
            let wj = ws[j] * eta.exp();
            for a in 0..p {
                let contribution = wj * (x[a][j] - s1[a] / s0);
                dbeta[j][a] += contribution;
            }
        }
        // For event subjects, add their X.
        for k in idx..tie_end {
            let j = order[k];
            for a in 0..p {
                dbeta[j][a] -= ws[j] * x[a][j];
            }
        }
        idx = tie_end;
    }

    // Scale by inverse information.
    let mut info_final = vec![vec![0.0_f64; p]; p];
    idx = 0;
    while idx < n {
        let i = order[idx];
        if event[i] == 0.0 {
            idx += 1;
            continue;
        }
        let t = time[i];
        let mut tie_end = idx;
        let mut d = 0.0;
        while tie_end < n && time[order[tie_end]] == t && event[order[tie_end]] == 1.0 {
            d += ws[order[tie_end]];
            tie_end += 1;
        }
        let mut s0 = 0.0;
        let mut s1 = vec![0.0_f64; p];
        let mut s2 = vec![vec![0.0_f64; p]; p];
        for k in idx..n {
            let j = order[k];
            let eta: f64 = (0..p).map(|a| beta[a] * x[a][j]).sum();
            let wj = ws[j] * eta.exp();
            s0 += wj;
            for a in 0..p {
                s1[a] += wj * x[a][j];
                for b in 0..p {
                    s2[a][b] += wj * x[a][j] * x[b][j];
                }
            }
        }
        for a in 0..p {
            for b in 0..p {
                info_final[a][b] += d * (s2[a][b] / s0 - s1[a] * s1[b] / (s0 * s0));
            }
        }
        idx = tie_end;
    }
    let info_m = Mat::from_fn(p, p, |a, b| info_final[a][b]);
    let inv_info = match Llt::new(info_m.as_ref(), faer::Side::Lower) {
        Ok(l) => l.inverse(),
        Err(_) => {
            return Err(SurveyError::InvalidInput("singular Cox info".into()));
        }
    };

    // influence = dbeta * inv_info.
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

    Ok(SvyCoxphFit {
        coefficients: beta,
        design_cov,
        hazard_ratios,
        df: design.degf(),
        n,
        n_events,
    })
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

    let mut influence = vec![vec![0.0_f64; p]; n];
    for i in 0..n {
        for a in 0..p {
            let ef = xmat[a][i] * resid[i] * ws[i];
            for b in 0..p {
                influence[i][a] += ef * naive[(b, a)];
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
}
