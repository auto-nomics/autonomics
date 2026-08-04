//! Variance estimation for survey designs.
//!
//! Implements the single-stage stratified cluster variance estimator
//! (R `svyCprod`), the foundational computation underneath `svymean`,
//! `svytotal`, `svyratio`, and the influence-function variance of `svyglm`.
//!
//! ## Algorithm
//!
//! For a variable `z` (e.g. the weighted, centered value `w·(y-ȳ)/Σw`):
//!
//! 1. Sum `z` within each (stratum, cluster) combination to get PSU totals.
//! 2. Subtract the stratum mean of PSU totals from each PSU total.
//! 3. For each stratum `h` with `n_h` PSUs:
//!    - `V_h = (n_h / (n_h - 1)) · fpc_h · Σ residuals²`
//!    - where `fpc_h = (N_h - n_h) / N_h` if FPC given, else 1.
//! 4. Total variance `V = Σ_h V_h`.

use std::collections::HashMap;

use crate::design::SurveyDesign;
use crate::error::{Result, SurveyError};

/// Compute the design-based variance of `Σ z_i` (single stage).
///
/// This is the direct port of R's `svyCprod` for one variable (scalar
/// result). For a multivariate input, the result is the `p×p` covariance
/// matrix — see [`svy_cprod_matrix`].
///
/// ## Arguments
/// - `z`: the per-observation scaled variable (e.g. `w·(y-ȳ)/Σw`).
/// - `design`: the survey design.
pub fn svy_cprod(z: &[f64], design: &SurveyDesign) -> Result<f64> {
    if z.len() != design.n_obs {
        return Err(SurveyError::LengthMismatch {
            context: "z vs design".into(),
            a: z.len(),
            b: design.n_obs,
        });
    }
    let m = svy_cprod_matrix(&[z.to_vec()], design)?;
    Ok(m[0][0])
}

/// Compute the `p×p` design-based covariance matrix for `p` variables.
///
/// Each entry `V[i][j]` is the design-based covariance of
/// `Σ z_i[k]` and `Σ z_j[k]`.
pub fn svy_cprod_matrix(zs: &[Vec<f64>], design: &SurveyDesign) -> Result<Vec<Vec<f64>>> {
    let p = zs.len();
    if p == 0 {
        return Err(SurveyError::EmptyInput("no variables".into()));
    }
    let n = design.n_obs;
    for z in zs {
        if z.len() != n {
            return Err(SurveyError::LengthMismatch {
                context: "z row vs design".into(),
                a: z.len(),
                b: n,
            });
        }
    }

    // ── Step 1: Collapse z over PSUs (stratum × cluster) ─────────────────
    //
    // For each unique (stratum, cluster) pair, sum z across all observations
    // in that PSU.  This produces one p-vector per PSU.
    //
    // We also track which stratum each PSU belongs to.

    // Map (stratum, cluster) → index into psu_totals
    let mut psu_key: Vec<(String, String)> = Vec::new();
    let mut psu_index: HashMap<(String, String), usize> = HashMap::new();
    // psu_totals[k] = p-vector of summed z values for PSU k
    let mut psu_totals: Vec<Vec<f64>> = Vec::new();

    for i in 0..n {
        let key = (design.strata[i].clone(), design.cluster[i].clone());
        match psu_index.get(&key) {
            Some(&idx) => {
                for j in 0..p {
                    psu_totals[idx][j] += zs[j][i];
                }
            }
            None => {
                let idx = psu_totals.len();
                psu_index.insert(key.clone(), idx);
                psu_key.push(key.clone());
                psu_totals.push(zs.iter().map(|z| z[i]).collect());
            }
        }
    }

    let n_psu = psu_totals.len();

    // ── Step 2: Compute stratum means and residuals ─────────────────────
    //
    // For each stratum, compute the mean of the PSU totals, then subtract.

    // Group PSU indices by stratum.
    let mut stratum_psus: HashMap<String, Vec<usize>> = HashMap::new();
    for (k, (stratum, _)) in psu_key.iter().enumerate() {
        stratum_psus
            .entry(stratum.clone())
            .or_default()
            .push(k);
    }

    // Compute stratum means.
    let mut stratum_mean: HashMap<String, Vec<f64>> = HashMap::new();
    for (stratum, psu_idxs) in &stratum_psus {
        let count = psu_idxs.len() as f64;
        let mut mean = vec![0.0_f64; p];
        for &idx in psu_idxs {
            for j in 0..p {
                mean[j] += psu_totals[idx][j];
            }
        }
        for j in 0..p {
            mean[j] /= count;
        }
        stratum_mean.insert(stratum.clone(), mean);
    }

    // Compute residuals: psu_total - stratum_mean.
    let mut residuals: Vec<Vec<f64>> = vec![vec![0.0; p]; n_psu];
    for k in 0..n_psu {
        let stratum = &psu_key[k].0;
        let mean = &stratum_mean[stratum];
        for j in 0..p {
            residuals[k][j] = psu_totals[k][j] - mean[j];
        }
    }

    // ── Step 3: Accumulate variance per stratum ─────────────────────────
    //
    // For stratum h with n_h PSUs:
    //   V_h = (n_h / (n_h-1)) × fpc_h × Σ_{k in h} r_k × r_k'
    //
    // where fpc_h = (N_h - n_h) / N_h if population size given, else 1.

    let mut v = vec![vec![0.0_f64; p]; p];

    // Track observed (post-subset) PSU count per stratum.
    let mut obs_n: HashMap<String, usize> = HashMap::new();
    for key in &psu_key {
        *obs_n.entry(key.0.clone()).or_insert(0) += 1;
    }

    // Track whether any lonely PSU was found.
    let mut lonely_count = 0;

    for (stratum, psu_idxs) in &stratum_psus {
        let original_n = design.n_psu.get(stratum).copied().unwrap_or(obs_n[stratum]);
        let this_n = original_n;

        if this_n <= 1 {
            lonely_count += 1;
            match design.lonely_psu {
                crate::design::LonelyPsu::Fail => {
                    return Err(SurveyError::LonelyPsu {
                        stratum: stratum.clone(),
                        hint: "use lonely_psu policy other than 'fail'".into(),
                    });
                }
                _ => continue,
            }
        }

        let df = this_n as f64 / (this_n - 1) as f64;
        let fpc = if let Some(&popsize) = design.fpc.popsize.get(stratum) {
            if popsize > 0.0 && popsize.is_finite() {
                (popsize - this_n as f64) / popsize
            } else {
                1.0
            }
        } else {
            1.0
        };
        let scale = df * fpc;

        for &idx in psu_idxs {
            let r = &residuals[idx];
            for j in 0..p {
                for l in 0..p {
                    v[j][l] += r[j] * r[l] * scale;
                }
            }
        }
    }

    // Lonely-PSU "average" adjustment: divide by (1 - fraction of lonely strata).
    if design.lonely_psu == crate::design::LonelyPsu::Average && lonely_count > 0 {
        let n_strata = stratum_psus.len();
        let factor = 1.0 - lonely_count as f64 / n_strata as f64;
        if factor > 0.0 {
            for j in 0..p {
                for l in 0..p {
                    v[j][l] /= factor;
                }
            }
        }
    }

    Ok(v)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::design::{LonelyPsu, SurveyDesignBuilder};

    // The R `fpc` test dataset (8 observations, 2 strata).
    // Golden values from R survey v4.5:
    //   svymean(~x, withoutfpc) → mean=5.448148, SE=0.7412683
    //   svymean(~x, withfpc)    → mean=5.448148, SE=0.6160407

    fn fpc_strata() -> Vec<String> {
        vec![
            "1".into(), "1".into(), "1".into(), "1".into(), "1".into(),
            "2".into(), "2".into(), "2".into(),
        ]
    }
    fn fpc_cluster() -> Vec<String> {
        // Already nested: stratum:psu
        vec![
            "1.1".into(), "1.2".into(), "1.3".into(), "1.4".into(), "1.5".into(),
            "2.1".into(), "2.2".into(), "2.3".into(),
        ]
    }
    fn fpc_weights() -> Vec<f64> {
        vec![3.0, 3.0, 3.0, 3.0, 3.0, 4.0, 4.0, 4.0]
    }
    fn fpc_x() -> Vec<f64> {
        vec![2.8, 4.1, 6.8, 6.8, 9.2, 3.7, 6.6, 4.2]
    }
    fn fpc_popsize() -> HashMap<String, f64> {
        let mut m = HashMap::new();
        m.insert("1".into(), 15.0);
        m.insert("2".into(), 12.0);
        m
    }

    /// Compute the variance of a weighted mean using svy_cprod.
    /// This mirrors R's svymean for one variable.
    fn weighted_mean_var(
        x: &[f64],
        design: &SurveyDesign,
    ) -> (f64, f64) {
        let w = design.weights();
        let psum: f64 = w.iter().sum();

        // Weighted mean.
        let mean: f64 = x.iter().zip(&w).map(|(&xi, &wi)| xi * wi).sum::<f64>() / psum;

        // Scaled centered variable: z = w * (x - mean) / psum
        let z: Vec<f64> = x.iter().zip(&w).map(|(&xi, &wi)| wi * (xi - mean) / psum).collect();

        let var = svy_cprod(&z, design).unwrap();
        (mean, var)
    }

    #[test]
    fn mean_matches_r() {
        let d = SurveyDesignBuilder::new()
            .strata(fpc_strata())
            .cluster(fpc_cluster())
            .weights(fpc_weights())
            .lonely_psu(LonelyPsu::Remove)
            .build()
            .unwrap();
        let (mean, _var) = weighted_mean_var(&fpc_x(), &d);
        assert!(
            (mean - 5.448148).abs() < 1e-5,
            "mean should be ~5.448148, got {mean}"
        );
    }

    #[test]
    fn variance_without_fpc_matches_r() {
        let d = SurveyDesignBuilder::new()
            .strata(fpc_strata())
            .cluster(fpc_cluster())
            .weights(fpc_weights())
            .lonely_psu(LonelyPsu::Remove)
            .build()
            .unwrap();
        let (_mean, var) = weighted_mean_var(&fpc_x(), &d);
        let se = var.sqrt();
        assert!(
            (se - 0.7412683).abs() < 1e-5,
            "SE without FPC should be ~0.7412683, got {se}"
        );
    }

    #[test]
    fn variance_with_fpc_matches_r() {
        let d = SurveyDesignBuilder::new()
            .strata(fpc_strata())
            .cluster(fpc_cluster())
            .weights(fpc_weights())
            .fpc_popsize(fpc_popsize())
            .lonely_psu(LonelyPsu::Remove)
            .build()
            .unwrap();
        let (_mean, var) = weighted_mean_var(&fpc_x(), &d);
        let se = var.sqrt();
        assert!(
            (se - 0.6160407).abs() < 1e-5,
            "SE with FPC should be ~0.6160407, got {se}"
        );
    }
}
