//! The MR-PRESSO algorithm — a faithful port of `R/mr_presso.R` (v1.0).
//!
//! The three components (global / outlier / distortion tests) and the
//! leave-one-out weighted regressions reproduce the R reference. Random
//! draws use R's exact RNG (see [`crate::rng`]) so empirical p-values match R
//! for a shared `set.seed`.

use crate::error::{MrpressoError, Result};
use crate::linalg::{wls_coef, wls_summary};
use crate::rng::Rng;

/// Per-row summary statistics for a single exposure or the outcome.
///
/// MR-PRESSO operates on a matrix of instrument rows: each row is one
/// instrumental variable (typically one SNP) carrying the outcome effect, one
/// or more exposure effects, and their standard errors.
#[derive(Debug, Clone)]
pub struct MrpressoInput {
    /// Outcome effect per instrument (`BetaOutcome`).
    pub beta_outcome: Vec<f64>,
    /// Exposure effects, one vector of length `n` per exposure (`BetaExposure`).
    pub beta_exposure: Vec<Vec<f64>>,
    /// Outcome standard error per instrument (`SdOutcome`).
    pub sd_outcome: Vec<f64>,
    /// Exposure standard errors, one vector of length `n` per exposure.
    pub sd_exposure: Vec<Vec<f64>>,
    /// Run the outlier test (`OUTLIERtest`).
    pub outlier_test: bool,
    /// Run the distortion test (`DISTORTIONtest`).
    pub distortion_test: bool,
    /// Significance threshold in `(0, 1]` (`SignifThreshold`).
    pub signif_threshold: f64,
    /// Number of simulated null-distribution draws (`NbDistribution`).
    pub nb_distribution: usize,
    /// RNG seed (`set.seed`).
    pub seed: u32,
    /// Optional instrument labels (one per row), used to name outlier indices.
    /// When absent, 1-based row indices are reported.
    pub row_labels: Option<Vec<String>>,
}

/// One row of the `Main MR results` table (raw or outlier-corrected).
#[derive(Debug, Clone)]
pub struct MrResultRow {
    /// Exposure name (column index in `beta_exposure`).
    pub exposure: usize,
    /// `"Raw"` or `"Outlier-corrected"`.
    pub analysis: &'static str,
    /// `Causal Estimate`.
    pub causal_estimate: f64,
    /// `Sd` (Std. Error).
    pub sd: f64,
    /// `T-stat`.
    pub t_stat: f64,
    /// `P-value`.
    pub p_value: f64,
}

/// Global test result.
#[derive(Debug, Clone)]
pub struct GlobalTest {
    /// Observed leave-one-out residual sum of squares.
    pub rss_obs: f64,
    /// Empirical p-value `count(RSS_exp > RSS_obs) / NbDistribution`.
    pub pvalue: f64,
}

/// One instrument row of the outlier test.
#[derive(Debug, Clone)]
pub struct OutlierRow {
    /// 1-based row index in the cleaned data.
    pub index: usize,
    /// Instrument label (if `row_labels` given), else the row index as a string.
    pub label: String,
    /// Observed squared residual `Dif²`.
    pub rss_obs: f64,
    /// Bonferroni-corrected empirical p-value `min(p * n, 1)`.
    pub pvalue: f64,
}

/// Distortion test result.
#[derive(Debug, Clone)]
pub struct DistortionTest {
    /// 1-based row indices flagged as outliers.
    pub outlier_indices: Vec<usize>,
    /// Outlier labels (if `row_labels` given).
    pub outlier_labels: Vec<String>,
    /// Status message when no/all outliers (mirrors R's `Outliers Indices`).
    pub status: String,
    /// Distortion coefficient in percent, one per exposure.
    pub coefficient: Vec<f64>,
    /// Empirical p-value.
    pub pvalue: Option<f64>,
}

/// Full MR-PRESSO result.
#[derive(Debug, Clone)]
pub struct MrpressoOutput {
    /// The `Main MR results` table (raw then outlier-corrected, per exposure).
    pub main_mr: Vec<MrResultRow>,
    /// Global test (always present).
    pub global: GlobalTest,
    /// Outlier test — present only when it ran (global test significant and
    /// `outlier_test` requested).
    pub outlier: Option<Vec<OutlierRow>>,
    /// Distortion test — present only when both `outlier_test` and
    /// `distortion_test` ran and outliers were found.
    pub distortion: Option<DistortionTest>,
    /// Whether an outlier-corrected MR estimate was computed (else NA rows).
    pub has_corrected_mr: bool,
}

// ─────────────────────────────────────────────────────────────────────────
// Leave-one-out weighted residual sum of squares
// ─────────────────────────────────────────────────────────────────────────

/// Weighted leave-one-out regression coefficient estimates.
///
/// Returns a `p × n` matrix in column-major order: column `i` holds the
/// weighted-least-squares (through the origin) coefficient vector estimated
/// from all rows **except** row `i`. Matches R's `CausalEstimate_LOO` from
/// `getRSS_LOO`.
fn loo_coefficients(xw: &[f64], n: usize, p: usize, yw: &[f64]) -> Vec<f64> {
    // xw: column-major n×p weighted design; yw: weighted outcome length n.
    let mut est = vec![0.0; p * n];
    for i in 0..n {
        // Build X[-i], Y[-i], W[-i] (already folded into xw/yw).
        let m = n - 1;
        let mut xm = vec![0.0; m * p];
        let mut ym = vec![0.0; m];
        let mut row = 0usize;
        for r in 0..n {
            if r == i {
                continue;
            }
            for e in 0..p {
                xm[e * m + row] = xw[e * n + r];
            }
            ym[row] = yw[r];
            row += 1;
        }
        // Weights were folded into xw/yw by multiplying by sqrt(w); the WLS
        // through the origin with unit weights on the rescaled data is
        // equivalent to the weighted regression on the original data.
        let ones = vec![1.0; m];
        match wls_coef(&xm, m, p, &ym, &ones) {
            Ok(beta) => {
                for e in 0..p {
                    est[e * n + i] = beta[e];
                }
            }
            Err(_) => {
                // rank-deficient leave-one-out set
                for e in 0..p {
                    est[e * n + i] = f64::NAN;
                }
            }
        }
    }
    est
}

/// `getRSS_LOO`: observed (weighted) residual sum of squares.
///
/// `xw`/`yw` are the sign-transformed, weight-rescaled observed data (column
/// `e` of `xw` = `E_obs[e] * sqrt(w)`). Returns `(rss, loo_coefs)` where
/// `loo_coefs` is the `p × n` leave-one-out coefficient matrix.
fn rss_loo(xw: &[f64], n: usize, p: usize, yw: &[f64]) -> (f64, Vec<f64>) {
    let loo = loo_coefficients(xw, n, p, yw);
    let mut rss = 0.0;
    for i in 0..n {
        let mut pred = 0.0;
        for e in 0..p {
            pred += xw[e * n + i] * loo[e * n + i];
        }
        let r = yw[i] - pred;
        rss += r * r;
    }
    (rss, loo)
}

// ─────────────────────────────────────────────────────────────────────────
// Random null data
// ─────────────────────────────────────────────────────────────────────────

/// Simulate one random dataset (`getRandomData`), drawing from `rng` in R's
/// order: all exposures first (per exposure, all rows), then the outcome.
///
/// * `e_obs`/`sd_e` — original observed exposures / exposure SEs (p × n).
/// * `y_obs`/`sd_y` — original observed outcome / outcome SE (n).
/// * `loo` — observed leave-one-out coefficients (p × n).
///
/// Returns the `(p + 1) × n` random matrix: rows 0..p are exposures, row p is
/// the outcome (the `Weights` column is appended by the caller if needed).
fn random_data(
    rng: &mut Rng,
    e_obs: &[f64],
    sd_e: &[f64],
    n: usize,
    p: usize,
    sd_y: &[f64],
    loo: &[f64],
) -> Vec<f64> {
    let mut out = vec![0.0; (p + 1) * n];
    // Exposures: rnorm(n, data[,E], data[,SdE]) — in column-major [p][n] layout.
    for e in 0..p {
        for i in 0..n {
            out[e * n + i] = rng.rnorm(e_obs[e * n + i], sd_e[e * n + i]);
        }
    }
    // Outcome: for each row i, rnorm(1, predict(loo_i, row i), data[i, SdY]).
    for i in 0..n {
        let mut pred = 0.0;
        for e in 0..p {
            pred += e_obs[e * n + i] * loo[e * n + i];
        }
        out[p * n + i] = rng.rnorm(pred, sd_y[i]);
    }
    out
}

// ─────────────────────────────────────────────────────────────────────────
// Main entry point
// ─────────────────────────────────────────────────────────────────────────

/// Run the full MR-PRESSO pipeline.
pub fn mr_presso(input: &MrpressoInput) -> Result<MrpressoOutput> {
    let p = input.beta_exposure.len();
    if p == 0 {
        return Err(MrpressoError::ExposureSdMismatch);
    }
    if input.sd_exposure.len() != p {
        return Err(MrpressoError::ExposureSdMismatch);
    }
    if input.signif_threshold > 1.0 {
        return Err(MrpressoError::ThresholdTooLarge(input.signif_threshold));
    }

    let n_in = input.beta_outcome.len();
    for (e, v) in input.beta_exposure.iter().enumerate() {
        if v.len() != n_in {
            return Err(MrpressoError::LengthMismatch(format!(
                "beta_exposure[{e}] len={} != n={n_in}",
                v.len()
            )));
        }
        if input.sd_exposure[e].len() != n_in {
            return Err(MrpressoError::LengthMismatch(format!(
                "sd_exposure[{e}] len={} != n={n_in}",
                input.sd_exposure[e].len()
            )));
        }
    }
    if input.sd_outcome.len() != n_in {
        return Err(MrpressoError::LengthMismatch(format!(
            "sd_outcome len={} != n={n_in}",
            input.sd_outcome.len()
        )));
    }

    // ── 0. Data prep (drop NA rows, sign-align on the first exposure) ──
    let mut keep = Vec::new();
    for i in 0..n_in {
        let mut ok = !input.beta_outcome[i].is_nan() && !input.sd_outcome[i].is_nan();
        for e in 0..p {
            ok &= !input.beta_exposure[e][i].is_nan() && !input.sd_exposure[e][i].is_nan();
        }
        if ok {
            keep.push(i);
        }
    }
    let n = keep.len();
    if n == 0 {
        return Err(MrpressoError::EmptyData);
    }

    let mut e_obs = vec![0.0; n * p]; // column-major [p][n]
    let mut sd_e = vec![0.0; n * p];
    let mut y_obs = vec![0.0; n];
    let mut sd_y = vec![0.0; n];
    let mut labels: Vec<String> = Vec::with_capacity(n);
    for (row, &orig) in keep.iter().enumerate() {
        let s = sign(input.beta_exposure[0][orig]); // sign of first exposure
        y_obs[row] = input.beta_outcome[orig] * s;
        sd_y[row] = input.sd_outcome[orig];
        for e in 0..p {
            e_obs[e * n + row] = input.beta_exposure[e][orig] * s;
            sd_e[e * n + row] = input.sd_exposure[e][orig];
        }
        if let Some(lbl) = &input.row_labels {
            labels.push(lbl[orig].clone());
        }
    }

    // Weights = 1 / sd_outcome².
    let mut w = vec![0.0; n];
    for i in 0..n {
        w[i] = 1.0 / (sd_y[i] * sd_y[i]);
    }

    // ── Validation (mirrors R's stops) ──
    if n <= p + 2 {
        return Err(MrpressoError::NotEnoughInstruments(n, p));
    }
    if n >= input.nb_distribution {
        return Err(MrpressoError::NotEnoughForEmpirical(
            n,
            input.nb_distribution,
        ));
    }

    // Weight-rescaled observed design (dataW = data * sqrt(w)).
    let mut xw = vec![0.0; n * p];
    let mut yw = vec![0.0; n];
    for i in 0..n {
        let sw = w[i].sqrt();
        yw[i] = y_obs[i] * sw;
        for e in 0..p {
            xw[e * n + i] = e_obs[e * n + i] * sw;
        }
    }

    // ── 1. Observed RSS ──
    let (rss_obs, loo_obs) = rss_loo(&xw, n, p, &yw);

    // ── 2. Empirical null distribution of RSS ──
    let mut rng = Rng::new(input.seed);
    let mut rss_exp = Vec::with_capacity(input.nb_distribution);
    let mut random_data_all: Vec<Vec<f64>> = Vec::with_capacity(input.nb_distribution);
    for _ in 0..input.nb_distribution {
        let rd = random_data(&mut rng, &e_obs, &sd_e, n, p, &sd_y, &loo_obs);
        // getRSS_LOO scales the simulated data by sqrt(Weights) internally.
        let mut rdx = rd[..n * p].to_vec();
        let mut rdy = rd[p * n..].to_vec();
        for i in 0..n {
            let sw = w[i].sqrt();
            rdy[i] *= sw;
            for e in 0..p {
                rdx[e * n + i] *= sw;
            }
        }
        let (rss, _) = rss_loo(&rdx, n, p, &rdy);
        rss_exp.push(rss);
        random_data_all.push(rd);
    }

    let global_pvalue =
        rss_exp.iter().filter(|&&r| r > rss_obs).count() as f64 / input.nb_distribution as f64;
    let global = GlobalTest {
        rss_obs,
        pvalue: global_pvalue,
    };

    // ── 3. Outlier test ──
    let outlier_effective = input.outlier_test && global_pvalue < input.signif_threshold;
    let mut outlier: Option<Vec<OutlierRow>> = None;
    let mut distortion: Option<DistortionTest> = None;

    if outlier_effective {
        let mut rows = Vec::with_capacity(n);
        for snv in 0..n {
            // Dif = Y_obs - sum_e E_obs · loo_obs[e, snv]   (unweighted data)
            let mut dif = y_obs[snv];
            for e in 0..p {
                dif -= e_obs[e * n + snv] * loo_obs[e * n + snv];
            }
            // Exp: over the NbDistribution random datasets at this row.
            let mut cnt = 0u64;
            for rd in &random_data_all {
                let mut exp = rd[p * n + snv]; // random outcome
                for e in 0..p {
                    exp -= rd[e * n + snv] * loo_obs[e * n + snv];
                }
                if exp * exp > dif * dif {
                    cnt += 1;
                }
            }
            let raw_p = cnt as f64 / input.nb_distribution as f64;
            // Bonferroni: min(p * n, 1).
            let pv = (raw_p * n as f64).min(1.0);
            let label = if labels.is_empty() {
                (snv + 1).to_string()
            } else {
                labels[snv].clone()
            };
            rows.push(OutlierRow {
                index: snv + 1,
                label,
                rss_obs: dif * dif,
                pvalue: pv,
            });
        }
        outlier = Some(rows);

        // ── 4. Distortion test ──
        if input.distortion_test {
            distortion = Some(distortion_test(
                &xw,
                n,
                p,
                &yw,
                &y_obs,
                &e_obs,
                &sd_y,
                &w,
                outlier.as_ref().unwrap(),
                input.nb_distribution,
                input.signif_threshold,
                &mut rng,
            ));
        }
    }

    // ── 5. Main MR results ──
    // mod_all on all rows (weighted, through origin) — raw data + weights.
    let all = wls_summary(&e_obs, n, p, &y_obs, &w)?;
    let mut main_mr: Vec<MrResultRow> = Vec::with_capacity(2 * p);
    for e in 0..p {
        main_mr.push(MrResultRow {
            exposure: e,
            analysis: "Raw",
            causal_estimate: all.coef[e],
            sd: all.se[e],
            t_stat: all.t_values()[e],
            p_value: all.p_values()[e],
        });
    }

    // Outlier-corrected estimate exists iff mod_noOutliers was computed.
    let mut has_corrected_mr = false;
    let mut corrected_summary = None;
    if let Some(dt) = &distortion {
        // mod_noOutliers is defined when outliers exist and are not all rows.
        if let Some(ref_out) = Some(&dt.outlier_indices) {
            if !ref_out.is_empty() && ref_out.len() < n {
                corrected_summary = Some(wls_summary_excluding(&e_obs, n, p, &y_obs, &w, ref_out)?);
                has_corrected_mr = true;
            }
        }
    }
    for e in 0..p {
        match &corrected_summary {
            Some(cs) => main_mr.push(MrResultRow {
                exposure: e,
                analysis: "Outlier-corrected",
                causal_estimate: cs.coef[e],
                sd: cs.se[e],
                t_stat: cs.t_values()[e],
                p_value: cs.p_values()[e],
            }),
            None => main_mr.push(MrResultRow {
                exposure: e,
                analysis: "Outlier-corrected",
                causal_estimate: f64::NAN,
                sd: f64::NAN,
                t_stat: f64::NAN,
                p_value: f64::NAN,
            }),
        }
    }

    Ok(MrpressoOutput {
        main_mr,
        global,
        outlier,
        distortion,
        has_corrected_mr,
    })
}

/// `sign()` as in R: −1 / 0 / +1.
fn sign(x: f64) -> f64 {
    if x > 0.0 {
        1.0
    } else if x < 0.0 {
        -1.0
    } else {
        0.0
    }
}

/// Fit the weighted-through-origin regression excluding `ref_out` rows
/// (`mod_noOutliers`), returning the `summary.lm`.
fn wls_summary_excluding(
    xw: &[f64],
    n: usize,
    p: usize,
    yw: &[f64],
    w: &[f64],
    ref_out: &[usize],
) -> Result<crate::linalg::WlsSummary> {
    let keep: Vec<usize> = (0..n).filter(|i| !ref_out.contains(&(i + 1))).collect();
    let m = keep.len();
    let mut xm = vec![0.0; m * p];
    let mut ym = vec![0.0; m];
    let mut wm = vec![0.0; m];
    for (row, &orig) in keep.iter().enumerate() {
        ym[row] = yw[orig];
        wm[row] = w[orig];
        for e in 0..p {
            xm[e * m + row] = xw[e * n + orig];
        }
    }
    wls_summary(&xm, m, p, &ym, &wm)
}

/// R's `getRandomBias` for the distortion test. Draws `k` random subsets (each
/// keeping the outliers plus a random sample of non-outliers), fits the
/// weighted regression, and returns `NbDistribution × p` coefficient matrix.
#[allow(clippy::too_many_arguments)]
fn distortion_test(
    xw: &[f64],
    n: usize,
    p: usize,
    yw: &[f64],
    y_obs: &[f64],
    e_obs: &[f64],
    _sd_y: &[f64],
    w: &[f64],
    outlier: &[OutlierRow],
    nb_distribution: usize,
    signif_threshold: f64,
    rng: &mut Rng,
) -> DistortionTest {
    let ref_out: Vec<usize> = outlier
        .iter()
        .filter(|r| r.pvalue <= signif_threshold)
        .map(|r| r.index)
        .collect();
    let non_out: Vec<usize> = (1..=n).filter(|i| !ref_out.contains(i)).collect();

    if ref_out.is_empty() {
        return DistortionTest {
            outlier_indices: vec![],
            outlier_labels: vec![],
            status: "No significant outliers".to_string(),
            coefficient: vec![],
            pvalue: None,
        };
    }
    if ref_out.len() >= n {
        return DistortionTest {
            outlier_indices: ref_out.clone(),
            outlier_labels: vec![],
            status: "All SNPs considered as outliers".to_string(),
            coefficient: vec![],
            pvalue: None,
        };
    }

    let k = n - ref_out.len();
    // BiasExp: NbDistribution × p (column-major: [e * nb + r]).
    let mut bias_exp = vec![0.0; p * nb_distribution];
    for r in 0..nb_distribution {
        // indices = c(refOutlier, replicate(k, sample(non_out)[1]))
        // The regression uses indices[1..k] = refOutlier ++ first (k - len_ref)
        // random non-outlier draws. Sample without replacement, take first.
        let mut taken: Vec<usize> = Vec::with_capacity(k);
        for _ in 0..k {
            // sample(non_out)[1] — first element of a random permutation.
            taken.push(sample_first(rng, &non_out));
        }
        // Build the regression subset indices (0-based), R's indices[1:k].
        // R's `refOutlier` and the sampled values are 1-based row numbers; we
        // store them 0-based for indexing into xw/e_obs.
        let ref0: Vec<usize> = ref_out.iter().map(|&i| i - 1).collect();
        let mut subset: Vec<usize> = Vec::with_capacity(k);
        subset.extend_from_slice(&ref0);
        for t in &taken {
            if subset.len() < k {
                subset.push(*t - 1); // sampled 1-based row → 0-based
            }
        }
        subset.truncate(k);
        let m = subset.len();
        let mut xm = vec![0.0; m * p];
        let mut ym = vec![0.0; m];
        let mut wm = vec![0.0; m];
        for (row, &orig) in subset.iter().enumerate() {
            ym[row] = yw[orig];
            wm[row] = w[orig];
            for e in 0..p {
                xm[e * m + row] = xw[e * n + orig];
            }
        }
        let ones = vec![1.0; m];
        if let Ok(beta) = wls_coef(&xm, m, p, &ym, &ones) {
            for e in 0..p {
                bias_exp[e * nb_distribution + r] = beta[e];
            }
        }
    }

    // Observed: mod_all (all data) vs mod_noOutliers — raw data + weights.
    let all = wls_summary(e_obs, n, p, y_obs, w).ok();
    let no_out = wls_summary_excluding(e_obs, n, p, y_obs, w, &ref_out).ok();
    let (coef_all, coef_no): (Vec<f64>, Vec<f64>) = match (&all, &no_out) {
        (Some(a), Some(b)) => (a.coef.clone(), b.coef.clone()),
        _ => {
            return DistortionTest {
                outlier_indices: ref_out.clone(),
                outlier_labels: vec![],
                status: "No significant outliers".to_string(),
                coefficient: vec![],
                pvalue: None,
            };
        }
    };

    let mut bias_obs = vec![0.0; p];
    for e in 0..p {
        bias_obs[e] = (coef_all[e] - coef_no[e]) / coef_no[e].abs();
    }

    // BiasExp <- (coef_all - BiasExp) / abs(BiasExp);  P = count(|.| > |BiasObs|).
    let mut cnt = 0u64;
    for e in 0..p {
        for r in 0..nb_distribution {
            let be = bias_exp[e * nb_distribution + r];
            let val = (coef_all[e] - be) / be.abs();
            if val.abs() > bias_obs[e].abs() {
                cnt += 1;
            }
        }
    }
    let pvalue = cnt as f64 / nb_distribution as f64;

    let outlier_labels: Vec<String> = if outlier.is_empty() {
        vec![]
    } else {
        ref_out
            .iter()
            .filter_map(|i| {
                outlier
                    .iter()
                    .find(|o| o.index == *i)
                    .map(|o| o.label.clone())
            })
            .collect()
    };

    DistortionTest {
        outlier_indices: ref_out.clone(),
        outlier_labels,
        status: "ok".to_string(),
        coefficient: bias_obs.iter().map(|b| 100.0 * b).collect(),
        pvalue: Some(pvalue),
    }
}

/// R's `sample(x)[1]`: first element of a random permutation of `x`.
///
/// R's `sample(x)` (prob = NULL, no replacement) computes a **full**
/// permutation (all `len(x)` draws, `R_unif_index` with decreasing `n`), then
/// `[1]` takes the first result. To reproduce the RNG stream exactly we must
/// consume all `m` draws, but only the first determines the sampled value.
fn sample_first(rng: &mut Rng, x: &[usize]) -> usize {
    let m = x.len();
    let first_j = rng.unif_index(m as f64) as usize;
    for nn in (1..m).rev() {
        let _ = rng.unif_index(nn as f64);
    }
    x[first_j]
}

#[cfg(test)]
mod tests {
    // (cross-validation lives in tests/cross_validation.rs)
}
