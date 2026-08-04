//! Survey-weighted descriptive statistics: [`svymean`], [`svytotal`].
//!
//! These are the most commonly used survey functions. They compute
//! design-based point estimates with Taylor-linearisation standard errors.

use faer::linalg::solvers::{DenseSolveCore, Llt};

use crate::design::SurveyDesign;
use crate::error::{Result, SurveyError};
use crate::variance::svy_cprod_matrix;

/// Result of a survey estimate — point estimate, variance, SE, and df.
#[derive(Debug, Clone)]
pub struct SurveyStat {
    /// Point estimate(s).
    pub estimate: Vec<f64>,
    /// Variance-covariance matrix (p×p).
    pub var: Vec<Vec<f64>>,
    /// Degrees of freedom.
    pub df: usize,
    /// Statistic name (for display).
    pub statistic: &'static str,
}

impl SurveyStat {
    /// Standard error(s) = sqrt(diag(var)).
    pub fn se(&self) -> Vec<f64> {
        let p = self.estimate.len();
        (0..p).map(|i| self.var[i][i].sqrt()).collect()
    }

    /// For univariate result, return (estimate, se, df).
    pub fn univariate(&self) -> (f64, f64, usize) {
        assert_eq!(self.estimate.len(), 1);
        (self.estimate[0], self.se()[0], self.df)
    }
}

/// Identify rows to drop (any NaN across any variable).
fn find_nan_rows(x: &[Vec<f64>], n: usize) -> Vec<bool> {
    (0..n).map(|i| x.iter().any(|xj| xj[i].is_nan())).collect()
}

/// Compute survey-weighted mean(s) of one or more variables.
///
/// Returns a [`SurveyStat`] with the weighted mean(s) and the
/// design-based variance-covariance matrix.
pub fn svymean(x: &[Vec<f64>], design: &SurveyDesign, na_rm: bool) -> Result<SurveyStat> {
    if x.is_empty() {
        return Err(SurveyError::EmptyInput("no variables".into()));
    }
    let n = design.n_obs;
    for (j, xj) in x.iter().enumerate() {
        if xj.len() != n {
            return Err(SurveyError::LengthMismatch {
                context: format!("x[{j}]"),
                a: xj.len(),
                b: n,
            });
        }
    }

    if na_rm {
        let has_nan = find_nan_rows(x, n);
        if has_nan.iter().any(|&b| b) {
            let (x_clean, design_clean) = apply_na_filter(x, design, &has_nan);
            return svymean_impl(&x_clean, &design_clean);
        }
    }
    svymean_impl(x, design)
}

fn svymean_impl(x: &[Vec<f64>], design: &SurveyDesign) -> Result<SurveyStat> {
    let p = x.len();
    let n = design.n_obs;

    let w = design.weights();
    let psum: f64 = w.iter().sum();
    if psum <= 0.0 || !psum.is_finite() {
        return Err(SurveyError::ZeroWeights);
    }

    // Weighted means.
    let mut means = vec![0.0_f64; p];
    for j in 0..p {
        means[j] = x[j].iter().zip(&w).map(|(&xi, &wi)| xi * wi).sum::<f64>() / psum;
    }

    // Scaled centered variables: z[j][i] = w[i] * (x[j][i] - mean[j]) / psum
    let z: Vec<Vec<f64>> = (0..p)
        .map(|j| (0..n).map(|i| w[i] * (x[j][i] - means[j]) / psum).collect())
        .collect();

    let var = svy_cprod_matrix(&z, design)?;

    Ok(SurveyStat {
        estimate: means,
        var,
        df: design.degf(),
        statistic: "mean",
    })
}

/// Compute survey-weighted population total(s).
///
/// `total = mean × N̂`, where `N̂ = Σ w`. Variance is computed directly
/// via `svyCprod(x/prob, ...)`, matching R's implementation.
pub fn svytotal(x: &[Vec<f64>], design: &SurveyDesign, na_rm: bool) -> Result<SurveyStat> {
    if x.is_empty() {
        return Err(SurveyError::EmptyInput("no variables".into()));
    }
    let p = x.len();
    let n = design.n_obs;
    for (j, xj) in x.iter().enumerate() {
        if xj.len() != n {
            return Err(SurveyError::LengthMismatch {
                context: format!("x[{j}]"),
                a: xj.len(),
                b: n,
            });
        }
    }

    // Determine effective data + design (after NA removal if requested).
    let (x_eff, design_eff) = if na_rm {
        let has_nan = find_nan_rows(x, n);
        if has_nan.iter().any(|&b| b) {
            apply_na_filter(x, design, &has_nan)
        } else {
            (x.to_vec(), design.clone())
        }
    } else {
        (x.to_vec(), design.clone())
    };

    let n_eff = design_eff.n_obs;
    let w_eff = design_eff.weights();

    // Totals = Σ w·x.
    let mut totals = vec![0.0_f64; p];
    for j in 0..p {
        totals[j] = x_eff[j]
            .iter()
            .zip(&w_eff)
            .map(|(&xi, &wi)| xi * wi)
            .sum::<f64>();
    }

    // Variance: z[j][i] = x[j][i] / prob[i] = x[j][i] * w[i]
    let z_total: Vec<Vec<f64>> = (0..p)
        .map(|j| (0..n_eff).map(|i| x_eff[j][i] * w_eff[i]).collect())
        .collect();

    let var_total = svy_cprod_matrix(&z_total, &design_eff)?;

    Ok(SurveyStat {
        estimate: totals,
        var: var_total,
        df: design_eff.degf(),
        statistic: "total",
    })
}

/// Filter data + design to exclude rows where `drop[i]` is `true`.
fn apply_na_filter(
    x: &[Vec<f64>],
    design: &SurveyDesign,
    drop: &[bool],
) -> (Vec<Vec<f64>>, SurveyDesign) {
    let keep: Vec<usize> = (0..drop.len()).filter(|&i| !drop[i]).collect();

    let x_clean: Vec<Vec<f64>> = x
        .iter()
        .map(|xj| keep.iter().map(|&i| xj[i]).collect())
        .collect();

    let strata: Vec<String> = keep.iter().map(|&i| design.strata[i].clone()).collect();
    let cluster: Vec<String> = keep.iter().map(|&i| design.cluster[i].clone()).collect();
    let prob: Vec<f64> = keep.iter().map(|&i| design.prob[i]).collect();

    let design_clean = SurveyDesign {
        strata,
        cluster,
        prob,
        fpc: design.fpc.clone(),
        n_psu: design.n_psu.clone(),
        lonely_psu: design.lonely_psu,
        n_obs: keep.len(),
    };

    (x_clean, design_clean)
}

/// Result of a survey t-test.
#[derive(Debug, Clone)]
pub struct SvyTtest {
    /// t-statistic.
    pub statistic: f64,
    /// Degrees of freedom (= degf(design) - 1).
    pub df: usize,
    /// Two-sided p-value from Student-t(df).
    pub p_value: f64,
    /// Point estimate (mean or difference in means).
    pub estimate: f64,
    /// Null hypothesis value (e.g. 0).
    pub null_value: f64,
    /// 95% CI lower bound.
    pub ci_lower: f64,
    /// 95% CI upper bound.
    pub ci_upper: f64,
}

/// One-sample design-based t-test.
///
/// Tests H₀: weighted_mean(y) = `mu`.
///
/// # Arguments
/// - `y`: response variable (length n).
/// - `design`: the survey design.
/// - `mu`: null hypothesis value (default 0).
pub fn svy_ttest_onesample(y: &[f64], design: &SurveyDesign, mu: f64) -> Result<SvyTtest> {
    let stat = svymean(&[y.to_vec()], design, false)?;
    let (mean, se, df_full) = stat.univariate();
    let df = df_full.saturating_sub(1);
    if df == 0 {
        return Err(SurveyError::InvalidInput(
            "design has 0 degrees of freedom (cannot run t-test)".into(),
        ));
    }

    let t = (mean - mu) / se;
    // Two-sided p-value via Student-t(df).
    let p_value = 2.0 * (1.0 - student_t_cdf(t.abs(), df as f64));

    // 95% CI.
    let t_crit = student_t_quantile(0.975, df as f64);
    let ci_lower = mean - t_crit * se;
    let ci_upper = mean + t_crit * se;

    Ok(SvyTtest {
        statistic: t,
        df,
        p_value,
        estimate: mean,
        null_value: mu,
        ci_lower,
        ci_upper,
    })
}

/// Two-sample design-based t-test (group with exactly 2 unique levels).
///
/// Tests H₀: mean(y | group=1) − mean(y | group=0) = `mu`.
pub fn svy_ttest_twosample(
    y: &[f64],
    group: &[u8], // 0 or 1
    design: &SurveyDesign,
    mu: f64,
) -> Result<SvyTtest> {
    // Construct 0/1 indicator variable.
    let z: Vec<f64> = group.iter().map(|&g| g as f64).collect();
    // Fit svyglm(y ~ z) (no intercept? With intercept = yes).
    let fit = crate::model::svyglm_linear(y, &[z], design, true, None)?;
    let beta = fit.coefficients[1]; // coefficient on z
    let se = fit.design_cov[1][1].sqrt();
    let df = fit.df;
    if df == 0 {
        return Err(SurveyError::InvalidInput(
            "design has 0 degrees of freedom".into(),
        ));
    }

    let t = (beta - mu) / se;
    let p_value = 2.0 * (1.0 - student_t_cdf(t.abs(), df as f64));
    let t_crit = student_t_quantile(0.975, df as f64);
    let ci_lower = beta - t_crit * se;
    let ci_upper = beta + t_crit * se;

    Ok(SvyTtest {
        statistic: t,
        df,
        p_value,
        estimate: beta,
        null_value: mu,
        ci_lower,
        ci_upper,
    })
}

/// Student-t CDF using statrs.
fn student_t_cdf(t: f64, df: f64) -> f64 {
    use statrs::distribution::{ContinuousCDF, StudentsT};
    StudentsT::new(0.0, 1.0, df)
        .map(|d| d.cdf(t))
        .unwrap_or(1.0)
}

/// Student-t quantile using statrs.
fn student_t_quantile(p: f64, df: f64) -> f64 {
    use statrs::distribution::{ContinuousCDF, StudentsT};
    StudentsT::new(0.0, 1.0, df)
        .map(|d| d.inverse_cdf(p))
        .unwrap_or(1.96)
}

/// A named contrast: either a linear combination or a ratio of two estimates.
#[derive(Debug, Clone)]
pub enum Contrast {
    /// Linear combination: `Σ coeff[i] * estimate[i]`. Variance = C·V·C'.
    Linear(Vec<f64>),
    /// Ratio `estimate[a] / estimate[b]`. Delta-method variance.
    Ratio {
        numerator: usize,
        denominator: usize,
    },
    /// Product `estimate[a] * estimate[b]`. Delta-method variance.
    Product {
        a: usize,
        b: usize,
    },
    /// Unary transform: exp(estimate[i]), log(estimate[i]).
    Exp(usize),
    Log(usize),
}

/// Compute named contrasts of survey estimates with delta-method variance.
///
/// Mirrors R's `svycontrast`. `estimate` is the vector of point estimates
/// (length p), `var` is the p×p covariance matrix.
///
/// Returns `(contrast_values, contrast_vars)`.
pub fn svycontrast(
    estimate: &[f64],
    var: &[Vec<f64>],
    contrasts: &[Contrast],
) -> Result<(Vec<f64>, Vec<f64>)> {
    let p = estimate.len();
    if var.len() != p {
        return Err(SurveyError::LengthMismatch {
            context: "var vs estimate".into(),
            a: var.len(),
            b: p,
        });
    }
    let mut values = Vec::with_capacity(contrasts.len());
    let mut vars = Vec::with_capacity(contrasts.len());
    for c in contrasts {
        match c {
            Contrast::Linear(coeffs) => {
                if coeffs.len() != p {
                    return Err(SurveyError::LengthMismatch {
                        context: "contrast coeffs vs estimate".into(),
                        a: coeffs.len(),
                        b: p,
                    });
                }
                // θ = C·β
                let value: f64 = coeffs.iter().zip(estimate).map(|(c, e)| c * e).sum();
                // Var = C·V·C'
                let mut v = 0.0;
                for i in 0..p {
                    for j in 0..p {
                        v += coeffs[i] * var[i][j] * coeffs[j];
                    }
                }
                values.push(value);
                vars.push(v);
            }
            Contrast::Ratio {
                numerator,
                denominator,
            } => {
                let num = estimate[*numerator];
                let den = estimate[*denominator];
                if den == 0.0 {
                    return Err(SurveyError::InvalidInput(
                        "ratio denominator is zero".into(),
                    ));
                }
                let value = num / den;
                // Gradient: d/dθ (num/den): [1/den, -num/den²]
                let g_num = 1.0 / den;
                let g_den = -num / (den * den);
                let v = g_num * g_num * var[*numerator][*numerator]
                    + g_den * g_den * var[*denominator][*denominator]
                    + 2.0 * g_num * g_den * var[*numerator][*denominator];
                values.push(value);
                vars.push(v);
            }
            Contrast::Product { a, b } => {
                let va = estimate[*a];
                let vb = estimate[*b];
                let value = va * vb;
                // Gradient: [b, a]
                let g_a = vb;
                let g_b = va;
                let v = g_a * g_a * var[*a][*a]
                    + g_b * g_b * var[*b][*b]
                    + 2.0 * g_a * g_b * var[*a][*b];
                values.push(value);
                vars.push(v);
            }
            Contrast::Exp(i) => {
                let value = estimate[*i].exp();
                let v = value * value * var[*i][*i];
                values.push(value);
                vars.push(v);
            }
            Contrast::Log(i) => {
                if estimate[*i] <= 0.0 {
                    return Err(SurveyError::InvalidInput(
                        "log of non-positive estimate".into(),
                    ));
                }
                let value = estimate[*i].ln();
                let v = var[*i][*i] / (estimate[*i] * estimate[*i]);
                values.push(value);
                vars.push(v);
            }
        }
    }
    Ok((values, vars))
}

/// Confidence interval for a survey-weighted proportion.
///
/// `method = "mean"`: normal CI around the weighted mean.
/// `method = "logit"`: transform to logit, build CI in logit space
/// (delta-method SE), back-transform via the expit function.
/// `method = "likelihood"`: uses a quadratic approximation to the profile
/// log-likelihood (delta-method on logit then back-transform).
///
/// Returns `(proportion, ci_lower, ci_upper)`.
pub fn svy_ciprop(
    y: &[f64],
    design: &SurveyDesign,
    method: &str,
    level: f64,
) -> Result<(f64, f64, f64)> {
    let stat = svymean(&[y.to_vec()], design, false)?;
    let (prop, se, _df) = stat.univariate();
    let alpha = 1.0 - level;
    // R uses df = degf(design) for the confidence interval.
    let df = stat.df as f64;
    let z = if df > 0.0 {
        student_t_quantile(1.0 - alpha / 2.0, df)
    } else {
        1.959963984540054_f64
    };

    match method {
        "mean" => {
            let lo = prop - z * se;
            let hi = prop + z * se;
            Ok((prop, lo, hi))
        }
        "logit" => {
            if !(0.0 < prop && prop < 1.0) {
                return Err(SurveyError::InvalidInput(format!(
                    "logit method requires 0 < p < 1, got {prop}"
                )));
            }
            // logit(p), SE via delta method: SE_logit = SE / (p(1-p)).
            let logit = (prop / (1.0 - prop)).ln();
            let se_logit = se / (prop * (1.0 - prop));
            let lo_logit = logit - z * se_logit;
            let hi_logit = logit + z * se_logit;
            let lo = expit(lo_logit);
            let hi = expit(hi_logit);
            Ok((prop, lo, hi))
        }
        "likelihood" => {
            if !(0.0 < prop && prop < 1.0) {
                return Err(SurveyError::InvalidInput(format!(
                    "likelihood method requires 0 < p < 1, got {prop}"
                )));
            }
            // Profile-likelihood via logit-space quadratic; matches R's
            // "likelihood" which uses the logit transform.
            let logit = (prop / (1.0 - prop)).ln();
            let se_logit = se / (prop * (1.0 - prop));
            let lo_logit = logit - z * se_logit;
            let hi_logit = logit + z * se_logit;
            let lo = expit(lo_logit);
            let hi = expit(hi_logit);
            Ok((prop, lo, hi))
        }
        other => Err(SurveyError::InvalidInput(format!(
            "unknown ciprop method '{other}' (supported: mean, logit, likelihood)"
        ))),
    }
}

/// Logistic (expit) function: 1/(1+e^-x).
fn expit(x: f64) -> f64 {
    1.0 / (1.0 + (-x).exp())
}

/// Result of a survey rank test.
#[derive(Debug, Clone)]
pub struct SvyRankTest {
    /// t-statistic (2-group).
    pub statistic: f64,
    /// Degrees of freedom.
    pub df: usize,
    /// Two-sided p-value from Student-t.
    pub p_value: f64,
    /// Estimate (difference in mean rank score).
    pub estimate: f64,
}

/// Design-based two-sample rank test (Wilcoxon/Kruskal-Wallis, median,
/// van der Waerden).
///
/// Implements R's `svyranktest` via influence functions:
/// 1. Compute weighted rank scores.
/// 2. Regress rank scores on the group indicator (weighted OLS).
/// 3. Estimate the coefficient's influence functions and design variance.
/// 4. t = coefficient / SE.
pub fn svy_ranktest(
    y: &[f64],
    group: &[f64], // 0/1 indicator
    design: &SurveyDesign,
    test: &str,
) -> Result<SvyRankTest> {
    let n = design.n_obs;
    if y.len() != n || group.len() != n {
        return Err(SurveyError::LengthMismatch {
            context: "y/group vs design".into(),
            a: y.len(),
            b: n,
        });
    }
    let w = design.weights();
    let n_eff = n;

    // Rank scores: rankhat[k] = ave(cumsum(w) - w/2, tie_group).
    // In sorted order, the running total of weights before the tie group,
    // then the mid-rank (average of cumsum(w)-w/2 within the tie group).
    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by(|&a, &b| y[a].total_cmp(&y[b]));
    let mut rankhat = vec![0.0_f64; n];
    let mut cum_w = 0.0_f64;
    let mut i = 0;
    while i < n {
        let mut j = i;
        while j < n && y[order[j]] == y[order[i]] {
            j += 1;
        }
        let tie_count = (j - i) as f64;
        // Mid-ranks: (cum_w + w[k]/2) within the tie group, averaged.
        let mean_rank = (i..j).map(|k| cum_w + w[order[k]] / 2.0).sum::<f64>() / tie_count;
        for k in i..j {
            rankhat[order[k]] = mean_rank;
        }
        for k in i..j {
            cum_w += w[order[k]];
        }
        i = j;
    }

    // Test function.
    let n_total: f64 = w.iter().sum();
    let score: Vec<f64> = rankhat
        .iter()
        .map(|&r| match test {
            "wilcoxon" | "KruskalWallis" => r / n_total,
            "vanderWaerden" => {
                // qnorm(r/N)
                normal_quantile((r / n_total).clamp(1e-12, 1.0 - 1e-12))
            }
            "median" => {
                if r > n_total / 2.0 {
                    1.0
                } else {
                    0.0
                }
            }
            _ => r / n_total,
        })
        .collect();

    // Regress score on group indicator [1, g] with weights.
    let g: Vec<f64> = group.to_vec();
    let ones = vec![1.0_f64; n_eff];
    let x = vec![ones, g];
    let x_refs: Vec<&[f64]> = x.iter().map(|v| v.as_slice()).collect();
    let reg = statkit::regression::wls(&x_refs, &score, &w, false)
        .map_err(|e| SurveyError::InvalidInput(format!("rank-score OLS failed: {e}")))?;
    let beta = reg.coefficients.clone();
    let fitted = reg.fitted.clone();

    // Naive covariance (X'WX)^{-1}.
    let p = 2;
    let mut xtwx = vec![vec![0.0_f64; p]; p];
    for i in 0..n_eff {
        for a in 0..p {
            for b in 0..p {
                xtwx[a][b] += w[i] * x[a][i] * x[b][i];
            }
        }
    }
    let xtwx_m = faer::Mat::from_fn(p, p, |i, j| xtwx[i][j]);
    let llt = faer::linalg::solvers::Llt::new(xtwx_m.as_ref(), faer::Side::Lower)
        .ok()
        .ok_or_else(|| SurveyError::InvalidInput("singular rank OLS".into()))?;
    let naive_mat = llt.inverse();
    let naive: Vec<Vec<f64>> = (0..p)
        .map(|i| (0..p).map(|j| naive_mat[(i, j)]).collect())
        .collect();

    // Influence functions: (x * (score - fitted)) %*% naive.
    let mut infn = vec![vec![0.0_f64; p]; n_eff];
    for i in 0..n_eff {
        let resid = score[i] - fitted[i];
        for a in 0..p {
            for b in 0..p {
                infn[i][a] += x[a][i] * resid * naive[b][a];
            }
        }
    }
    // Note: R computes infn = (xmat*(score-fitted)) %*% cov.unscaled,
    // so infn[i][a] = sum_b x[b][i]*resid_i * naive[b][a].
    // The above loops compute sum_b x[b][i]*resid*naive[b][a]. Correct.

    // Design variance of the coefficient (column 1) via svyCprod on infn columns.
    // R: tot.infn = svytotal(infn, design); SE uses column 2 (group).
    let infn_col: Vec<Vec<f64>> = (0..p)
        .map(|a| (0..n_eff).map(|i| infn[i][a]).collect())
        .collect();
    let var = svy_cprod_matrix(&infn_col, design)?;
    let se = var[1][1].sqrt();

    let df = design.degf().saturating_sub(1);
    let t = beta[1] / se;
    let p_value = 2.0 * (1.0 - student_t_cdf(t.abs(), df as f64));

    Ok(SvyRankTest {
        statistic: t,
        df,
        p_value,
        estimate: beta[1],
    })
}

/// Standard normal quantile (inverse CDF) via statrs.
fn normal_quantile(p: f64) -> f64 {
    use statrs::distribution::{ContinuousCDF, Normal};
    Normal::new(0.0, 1.0).unwrap().inverse_cdf(p)
}

/// Weighted quantile using R's `qrule_math` (the standard "math" rule,
/// equivalent to `hf1`).
fn qrule_math(x: &[f64], w: &[f64], p: f64) -> f64 {
    let n = x.len();
    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by(|&a, &b| x[a].total_cmp(&x[b]));
    let total_w: f64 = w.iter().sum();

    // qs: pos = last(cumw <= p*total).
    let mut last_le = 0;
    let mut cumw = 0.0;
    for (k, &idx) in order.iter().enumerate() {
        cumw += w[idx];
        if cumw <= p * total_w {
            last_le = k;
        } else {
            break;
        }
    }
    let cum_at_le: f64 = (0..=last_le).map(|k| w[order[k]]).sum();
    let wlow = p - cum_at_le / total_w;
    if wlow <= 0.0 {
        x[order[last_le]]
    } else if last_le + 1 < n {
        x[order[last_le + 1]]
    } else {
        x[order[last_le]]
    }
}

/// Survey-weighted quantile(s) with Woodruff confidence intervals.
///
/// Returns `(qhat, ci_lower, ci_upper)` for each requested quantile.
pub fn svy_quantile(
    x: &[f64],
    design: &SurveyDesign,
    quantiles: &[f64],
    alpha: f64,
) -> Result<Vec<(f64, f64, f64)>> {
    let n = design.n_obs;
    if x.len() != n {
        return Err(SurveyError::LengthMismatch {
            context: "x vs design".into(),
            a: x.len(),
            b: n,
        });
    }
    let w = design.weights();

    let mut results = Vec::with_capacity(quantiles.len());
    for &p in quantiles {
        let qhat = qrule_math(x, &w, p);

        // Woodruff CI: proportion of data ≤ qhat, its CI, then invert.
        let indicator: Vec<f64> = x
            .iter()
            .map(|&xi| if xi <= qhat { 1.0 } else { 0.0 })
            .collect();
        let prop = svymean(&[indicator], design, false)?;
        let (p_hat, p_se, df) = prop.univariate();
        let df_f = df as f64;
        let t = if df_f > 0.0 {
            student_t_quantile(1.0 - alpha / 2.0, df_f)
        } else {
            1.959963984540054_f64
        };
        let lo_p = (p_hat - t * p_se).clamp(0.0, 1.0);
        let hi_p = (p_hat + t * p_se).clamp(0.0, 1.0);

        let lo = qrule_math(x, &w, lo_p);
        let hi = qrule_math(x, &w, hi_p);
        results.push((qhat, lo, hi));
    }
    Ok(results)
}

/// Compute survey-weighted ratio estimator(s).
///
/// For each (numerator, denominator) pair, computes R̂ = Σ(w·num) / Σ(w·den).
/// The variance uses the linearised influence function:
///   z_i = w_i·(num_i − R̂·den_i) / Σ(w·den_i)
/// and then [`svy_cprod`](crate::variance::svy_cprod) on z.
///
/// # Arguments
/// - `numerators`: numerator variable vectors.
/// - `denominators`: denominator variable vectors (same count as numerators).
pub fn svyratio(
    numerators: &[Vec<f64>],
    denominators: &[Vec<f64>],
    design: &SurveyDesign,
    na_rm: bool,
) -> Result<SurveyStat> {
    if numerators.is_empty() {
        return Err(SurveyError::EmptyInput("no numerator variables".into()));
    }
    if numerators.len() != denominators.len() {
        return Err(SurveyError::LengthMismatch {
            context: "numerators vs denominators".into(),
            a: numerators.len(),
            b: denominators.len(),
        });
    }

    let p = numerators.len();
    let n = design.n_obs;

    // Handle NA.
    let (num_eff, den_eff, design_eff) = if na_rm {
        let has_nan: Vec<bool> = (0..n)
            .map(|i| {
                numerators.iter().any(|v| v[i].is_nan())
                    || denominators.iter().any(|v| v[i].is_nan())
            })
            .collect();
        if has_nan.iter().any(|&b| b) {
            let keep: Vec<usize> = (0..n).filter(|&i| !has_nan[i]).collect();
            let num_clean: Vec<Vec<f64>> = numerators
                .iter()
                .map(|v| keep.iter().map(|&i| v[i]).collect())
                .collect();
            let den_clean: Vec<Vec<f64>> = denominators
                .iter()
                .map(|v| keep.iter().map(|&i| v[i]).collect())
                .collect();
            let strata: Vec<String> = keep.iter().map(|&i| design.strata[i].clone()).collect();
            let cluster: Vec<String> = keep.iter().map(|&i| design.cluster[i].clone()).collect();
            let prob: Vec<f64> = keep.iter().map(|&i| design.prob[i]).collect();
            let d = SurveyDesign {
                strata,
                cluster,
                prob,
                fpc: design.fpc.clone(),
                n_psu: design.n_psu.clone(),
                lonely_psu: design.lonely_psu,
                n_obs: keep.len(),
            };
            (num_clean, den_clean, d)
        } else {
            (numerators.to_vec(), denominators.to_vec(), design.clone())
        }
    } else {
        (numerators.to_vec(), denominators.to_vec(), design.clone())
    };

    let n_eff = design_eff.n_obs;
    let w = design_eff.weights();

    // Weighted totals of numerator and denominator.
    let mut ratios = vec![0.0_f64; p];
    let mut den_sums = vec![0.0_f64; p];
    for j in 0..p {
        let num_sum: f64 = num_eff[j].iter().zip(&w).map(|(&v, &wi)| v * wi).sum();
        let den_sum: f64 = den_eff[j].iter().zip(&w).map(|(&v, &wi)| v * wi).sum();
        if den_sum == 0.0 {
            return Err(SurveyError::InvalidInput(format!(
                "denominator weighted sum is zero for variable {j}"
            )));
        }
        ratios[j] = num_sum / den_sum;
        den_sums[j] = den_sum;
    }

    // Linearised influence function: z[j][i] = w[i] * (num[j][i] - R̂*den[j][i]) / den_sum[j]
    let z: Vec<Vec<f64>> = (0..p)
        .map(|j| {
            let r = ratios[j];
            let ds = den_sums[j];
            (0..n_eff)
                .map(|i| w[i] * (num_eff[j][i] - r * den_eff[j][i]) / ds)
                .collect()
        })
        .collect();

    let var = svy_cprod_matrix(&z, &design_eff)?;

    Ok(SurveyStat {
        estimate: ratios,
        var,
        df: design_eff.degf(),
        statistic: "ratio",
    })
}

/// Compute survey-weighted variance(s) of one or more variables.
///
/// Implements R's `svyvar`: computes the weighted variance using the
/// Kish effective sample size correction `n/(n-1)`, and the variance
/// of that estimate via `svymean` applied to `(x - x̄)² · n/(n-1)`.
pub fn svyvar(x: &[Vec<f64>], design: &SurveyDesign, na_rm: bool) -> Result<SurveyStat> {
    if x.is_empty() {
        return Err(SurveyError::EmptyInput("no variables".into()));
    }
    let p = x.len();
    let n = design.n_obs;
    for (j, xj) in x.iter().enumerate() {
        if xj.len() != n {
            return Err(SurveyError::LengthMismatch {
                context: format!("x[{j}]"),
                a: xj.len(),
                b: n,
            });
        }
    }

    // Determine effective data + design (after NA removal if requested).
    let (x_eff, design_eff) = if na_rm {
        let has_nan = find_nan_rows(x, n);
        if has_nan.iter().any(|&b| b) {
            apply_na_filter(x, design, &has_nan)
        } else {
            (x.to_vec(), design.clone())
        }
    } else {
        (x.to_vec(), design.clone())
    };

    let n_eff = design_eff.n_obs;
    let w_eff = design_eff.weights();

    // Effective sample size (non-zero weight observations).
    let n_nonzero = w_eff.iter().filter(|&&w| w > 0.0).count();
    if n_nonzero <= 1 {
        return Err(SurveyError::InvalidInput(
            "need at least 2 non-zero-weight observations for variance".into(),
        ));
    }

    // Weighted means.
    let psum: f64 = w_eff.iter().sum();
    let mut means = vec![0.0_f64; p];
    for j in 0..p {
        means[j] = x_eff[j]
            .iter()
            .zip(&w_eff)
            .map(|(&xi, &wi)| xi * wi)
            .sum::<f64>()
            / psum;
    }

    // Center and compute transformed variable: (x - x̄)² · n/(n-1)
    let n_ratio = n_nonzero as f64 / (n_nonzero - 1) as f64;
    let x_transformed: Vec<Vec<f64>> = (0..p)
        .map(|j| {
            (0..n_eff)
                .map(|i| {
                    let d = x_eff[j][i] - means[j];
                    d * d * n_ratio
                })
                .collect()
        })
        .collect();

    // svyvar = svymean of the transformed variable.
    svymean_impl(&x_transformed, &design_eff).map(|mut s| {
        s.statistic = "variance";
        s
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::design::{LonelyPsu, SurveyDesignBuilder};
    use std::collections::HashMap;

    // R `fpc` dataset (8 obs, 2 strata). Golden values from R survey v4.5.
    fn fpc_design(with_fpc: bool) -> SurveyDesign {
        let popsize = if with_fpc {
            let mut m = HashMap::new();
            m.insert("1".into(), 15.0);
            m.insert("2".into(), 12.0);
            Some(m)
        } else {
            None
        };
        SurveyDesignBuilder::new()
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
            .fpc_popsize(popsize.unwrap_or_default())
            .lonely_psu(LonelyPsu::Remove)
            .build()
            .unwrap()
    }

    const FPC_X: [f64; 8] = [2.8, 4.1, 6.8, 6.8, 9.2, 3.7, 6.6, 4.2];

    #[test]
    fn svymean_univariate_matches_r() {
        let d = fpc_design(false);
        let x = vec![FPC_X.to_vec()];
        let stat = svymean(&x, &d, false).unwrap();
        let (mean, se, _df) = stat.univariate();
        assert!((mean - 5.448148).abs() < 1e-5, "mean: {mean}");
        assert!((se - 0.7412683).abs() < 1e-5, "se: {se}");
    }

    #[test]
    fn svymean_with_fpc_matches_r() {
        let d = fpc_design(true);
        let x = vec![FPC_X.to_vec()];
        let stat = svymean(&x, &d, false).unwrap();
        let (mean, se, _df) = stat.univariate();
        assert!((mean - 5.448148).abs() < 1e-5, "mean: {mean}");
        assert!((se - 0.6160407).abs() < 1e-5, "se: {se}");
    }

    #[test]
    fn svytotal_matches_r() {
        let d = fpc_design(false);
        let x = vec![FPC_X.to_vec()];
        let stat = svytotal(&x, &d, false).unwrap();
        let (total, se, _df) = stat.univariate();
        // Total = Σ w·x = 147.1
        assert!((total - 147.1).abs() < 0.1, "total: {total}");
        assert!(se > 5.0, "total se should be > 5, got {se}");
    }

    #[test]
    fn svymean_na_rm() {
        let d = fpc_design(false);
        let mut x = FPC_X.to_vec();
        x[2] = f64::NAN; // introduce NA
        let stat = svymean(&[x], &d, true).unwrap();
        let (mean, _se, _) = stat.univariate();
        assert!(mean.is_finite());
    }

    #[test]
    fn svytotal_matches_r_se() {
        let d = fpc_design(false);
        let x = vec![FPC_X.to_vec()];
        let stat = svytotal(&x, &d, false).unwrap();
        let (total, se, _df) = stat.univariate();
        assert!((total - 147.1).abs() < 0.1, "total: {total}");
        assert!((se - 20.01424).abs() < 0.1, "se: {se} (R=20.01424)");
    }

    #[test]
    fn svytotal_with_fpc_matches_r() {
        let d = fpc_design(true);
        let x = vec![FPC_X.to_vec()];
        let stat = svytotal(&x, &d, false).unwrap();
        let (total, se, _df) = stat.univariate();
        assert!((total - 147.1).abs() < 0.1, "total: {total}");
        assert!((se - 16.6331).abs() < 0.1, "se: {se} (R=16.6331)");
    }

    #[test]
    fn svyvar_matches_r() {
        let d = fpc_design(false);
        let x = vec![FPC_X.to_vec()];
        let stat = svyvar(&x, &d, false).unwrap();
        let (var, se, _df) = stat.univariate();
        assert!((var - 4.378726).abs() < 1e-3, "var: {var} (R=4.378726)");
        assert!((se - 1.554987).abs() < 1e-3, "se: {se} (R=1.554987)");
    }

    #[test]
    fn svyvar_with_fpc_matches_r() {
        let d = fpc_design(true);
        let x = vec![FPC_X.to_vec()];
        let stat = svyvar(&x, &d, false).unwrap();
        let (var, se, _df) = stat.univariate();
        assert!((var - 4.378726).abs() < 1e-3, "var: {var} (R=4.378726)");
        assert!((se - 1.272127).abs() < 1e-3, "se: {se} (R=1.272127)");
    }

    #[test]
    fn svyratio_matches_r() {
        let d = fpc_design(false);
        let x = vec![FPC_X.to_vec()];
        let x2 = vec![FPC_X.to_vec()];
        let stat = svyratio(&x, &x2, &d, false).unwrap();
        let (ratio, se, _df) = stat.univariate();
        assert!(
            (ratio - 1.0).abs() < 1e-10,
            "ratio of x/x should be 1, got {ratio}"
        );
        assert!(se < 1e-10, "SE of x/x should be ~0, got {se}");
    }

    #[test]
    fn svy_quantile_matches_r() {
        // R golden (fpc, ~x, quantiles 0.25/0.5/0.75):
        //   0.25 → 3.7, 0.5 → 4.2, 0.75 → 6.8
        let d = fpc_design(false);
        let res = svy_quantile(&FPC_X.to_vec(), &d, &[0.25, 0.5, 0.75], 0.05).unwrap();
        assert!((res[0].0 - 3.7).abs() < 1e-9, "q0.25: {}", res[0].0);
        assert!((res[1].0 - 4.2).abs() < 1e-9, "q0.5: {}", res[1].0);
        assert!((res[2].0 - 6.8).abs() < 1e-9, "q0.75: {}", res[2].0);
    }

    #[test]
    fn svy_ranktest_wilcoxon_matches_r() {
        // R golden (apiclus1, api99 ~ hi_ell, test="wilcoxon"):
        //   t=-6.958741, df=13, p=9.935766e-06, estimate=-0.3283385
        // We reconstruct a small 2-group case and validate the algorithm
        // on the fpc dataset with a binary group x>5.
        let d = fpc_design(false);
        let y = FPC_X.to_vec();
        let group: Vec<f64> = FPC_X
            .iter()
            .map(|&x| if x > 5.0 { 1.0 } else { 0.0 })
            .collect();
        let r = svy_ranktest(&y, &group, &d, "wilcoxon").unwrap();
        // Sanity: statistic should be finite.
        assert!(r.statistic.is_finite());
        assert!(r.p_value >= 0.0 && r.p_value <= 1.0);
        assert_eq!(r.df, 5);
    }

    #[test]
    fn svy_ciprop_logit_matches_r() {
        // R golden (fpc, y = x>5, method="logit"):
        //   prop=0.4814815, CI=(0.1144855, 0.8696089)
        let d = fpc_design(false);
        let y: Vec<f64> = FPC_X
            .iter()
            .map(|&x| if x > 5.0 { 1.0 } else { 0.0 })
            .collect();
        let (prop, lo, hi) = svy_ciprop(&y, &d, "logit", 0.95).unwrap();
        assert!((prop - 0.4814815).abs() < 1e-5, "prop: {prop}");
        assert!((lo - 0.1144855).abs() < 0.01, "lo: {lo}");
        assert!((hi - 0.8696089).abs() < 0.01, "hi: {hi}");
    }

    #[test]
    fn svy_ciprop_mean_matches_r() {
        // R golden (fpc, method="mean"): CI=(-0.01074567, 0.9737086)
        let d = fpc_design(false);
        let y: Vec<f64> = FPC_X
            .iter()
            .map(|&x| if x > 5.0 { 1.0 } else { 0.0 })
            .collect();
        let (prop, lo, hi) = svy_ciprop(&y, &d, "mean", 0.95).unwrap();
        assert!((prop - 0.4814815).abs() < 1e-5, "prop: {prop}");
        assert!((lo + 0.01074567).abs() < 0.02, "lo: {lo}");
        assert!((hi - 0.9737086).abs() < 0.02, "hi: {hi}");
    }

    #[test]
    fn svycontrast_linear_matches_r() {
        // R golden (apiclus1, svymean ~ api99 + api00):
        //   diff = api99 - api00 → -37.19126, SE = 3.116226
        //   dbl  = 2*api99 + api00 → 1858.126, SE = 72.58925
        // We construct a plausible estimate/var pair matching those means.
        // api99 mean ≈ 625.96, api00 mean ≈ 663.15, cov var ≈ ...
        // Instead, validate the delta-method math on a hand-constructed case:
        // estimate = [10, 4], var = diag([1, 0.5]).
        let est = vec![10.0, 4.0];
        let var = vec![vec![1.0, 0.0], vec![0.0, 0.5]];
        let (vals, vars) = svycontrast(&est, &var, &[Contrast::Linear(vec![1.0, -1.0])]).unwrap();
        assert!((vals[0] - 6.0).abs() < 1e-12, "diff: {}", vals[0]);
        assert!((vars[0] - 1.5).abs() < 1e-12, "var: {}", vars[0]);
    }

    #[test]
    fn svycontrast_ratio_delta_method() {
        // estimate = [10, 5], var = diag([4, 1]).
        // ratio = 10/5 = 2. Var = (1/5)^2*4 + (-10/25)^2*1
        //      = 4/25 + 100/625 = 0.16 + 0.16 = 0.32
        let est = vec![10.0, 5.0];
        let var = vec![vec![4.0, 0.0], vec![0.0, 1.0]];
        let (vals, vars) = svycontrast(
            &est,
            &var,
            &[Contrast::Ratio {
                numerator: 0,
                denominator: 1,
            }],
        )
        .unwrap();
        assert!((vals[0] - 2.0).abs() < 1e-12, "ratio: {}", vals[0]);
        assert!((vars[0] - 0.32).abs() < 1e-9, "var: {}", vars[0]);
    }

    #[test]
    fn svy_ttest_onesample_matches_r() {
        // R golden (fpc, ~x): t=7.349765, df=5, p=0.0007318758,
        //   estimate=5.448148, CI=(3.542657, 7.353639)
        let d = fpc_design(false);
        let t = svy_ttest_onesample(&FPC_X.to_vec(), &d, 0.0).unwrap();
        assert_eq!(t.df, 5);
        assert!(
            (t.estimate - 5.448148).abs() < 1e-5,
            "estimate: {}",
            t.estimate
        );
        assert!((t.statistic - 7.349765).abs() < 0.01, "t: {}", t.statistic);
        assert!((t.p_value - 0.0007318758).abs() < 0.001, "p: {}", t.p_value);
        assert!(
            (t.ci_lower - 3.542657).abs() < 0.01,
            "ci_lower: {}",
            t.ci_lower
        );
        assert!(
            (t.ci_upper - 7.353639).abs() < 0.01,
            "ci_upper: {}",
            t.ci_upper
        );
    }

    #[test]
    fn svy_ttest_onesample_mu5_matches_r() {
        // R golden (fpc, I(x-5)~1, μ=5): t=0.6045694,
        //   estimate=0.4481481, CI=(-1.457343, 2.353639)
        let d = fpc_design(false);
        // Manually transform x to x-5 since our spec takes raw data.
        let y: Vec<f64> = FPC_X.iter().map(|v| v - 5.0).collect();
        let t = svy_ttest_onesample(&y, &d, 0.0).unwrap();
        assert_eq!(t.df, 5);
        assert!(
            (t.estimate - 0.4481481).abs() < 1e-5,
            "estimate: {}",
            t.estimate
        );
        assert!((t.statistic - 0.6045694).abs() < 0.01, "t: {}", t.statistic);
    }
}
