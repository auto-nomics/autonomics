//! Calibration and weight adjustment for survey designs.
//!
//! Implements post-stratification, raking (iterative proportional fitting),
//! and generalized calibration against auxiliary population totals.
//!
//! All functions return a new [`SurveyDesign`] with updated sampling
//! weights (i.e., updated `prob` = 1 / new_weight).

use std::collections::HashMap;

use crate::design::SurveyDesign;
use crate::error::{Result, SurveyError};

/// Post-stratify a survey design against known population totals.
///
/// For each observation `i` with stratum label `s_i`, the new sampling
/// weight is `w_i * pop_count[s_i] / sample_count[s_i]`.
///
/// # Arguments
/// - `design`: original survey design.
/// - `strata`: stratum label per observation (length `design.n_obs`).
/// - `population`: `{level: population_count}` map.
/// - `partial`: if `true`, drop strata present in sample but absent from
///   population (R `partial = TRUE`).
pub fn post_stratify(
    design: &SurveyDesign,
    strata: &[String],
    population: &HashMap<String, f64>,
    partial: bool,
) -> Result<SurveyDesign> {
    if strata.len() != design.n_obs {
        return Err(SurveyError::LengthMismatch {
            context: "strata vs design".into(),
            a: strata.len(),
            b: design.n_obs,
        });
    }

    // Aggregate sample weights per stratum.
    let mut sample_count: HashMap<String, f64> = HashMap::new();
    let weights = design.weights();
    for (i, s) in strata.iter().enumerate() {
        *sample_count.entry(s.clone()).or_insert(0.0) += weights[i];
    }

    // Validate: every stratum in sample should have a population count.
    let mut reweight_per_obs: Vec<f64> = vec![1.0; design.n_obs];
    let mut new_weights: HashMap<String, f64> = HashMap::new();

    for (i, s) in strata.iter().enumerate() {
        let pop = match population.get(s) {
            Some(&p) if p > 0.0 => p,
            _ => {
                if !partial {
                    return Err(SurveyError::InvalidDesign(format!(
                        "stratum '{s}' has no population count (use partial=true to ignore)"
                    )));
                }
                continue;
            }
        };
        let samp = sample_count.get(s).copied().unwrap_or(0.0);
        if samp == 0.0 {
            if !partial {
                return Err(SurveyError::InvalidDesign(format!(
                    "stratum '{s}' has zero sample weight"
                )));
            }
            continue;
        }
        let ratio = pop / samp;
        new_weights.insert(s.clone(), ratio);
        reweight_per_obs[i] = ratio;
    }

    // Apply ratio to each observation's weight.
    let mut new_prob = design.prob.clone();
    for i in 0..design.n_obs {
        // If ratio is 1.0 (no calibration for this stratum), keep weight.
        if (reweight_per_obs[i] - 1.0).abs() > 1e-12 {
            // new_weight = old_weight * ratio = (1/prob) * ratio
            // → new_prob = 1 / (old_prob^{-1} * ratio) = old_prob / ratio
            new_prob[i] = design.prob[i] / reweight_per_obs[i];
        }
    }

    Ok(SurveyDesign {
        strata: design.strata.clone(),
        cluster: design.cluster.clone(),
        prob: new_prob,
        fpc: design.fpc.clone(),
        n_psu: design.n_psu.clone(),
        lonely_psu: design.lonely_psu,
        n_obs: design.n_obs,
    })
}

/// Rake (iterative proportional fitting) a survey design against multiple
/// marginal distributions.
///
/// Given a list of marginal variables `strata_marginals` and population
/// totals for each, iteratively adjust weights so that weighted marginal
/// totals match the population totals.
///
/// # Arguments
/// - `design`: original survey design.
/// - `strata_marginals`: list of `(column, population)` pairs.
/// - `maxit`: maximum iterations.
/// - `epsilon`: convergence threshold (max absolute change in any cell count).
pub fn rake(
    design: &SurveyDesign,
    strata_marginals: &[(Vec<String>, HashMap<String, f64>)],
    maxit: usize,
    epsilon: f64,
) -> Result<SurveyDesign> {
    if strata_marginals.is_empty() {
        return Err(SurveyError::InvalidInput("no margins to rake on".into()));
    }

    let n = design.n_obs;
    let mut weights = design.weights();

    for iter in 0..maxit {
        let mut max_change = 0.0;
        for (col, population) in strata_marginals {
            // Compute weighted sample totals per level of this margin.
            let mut sample_count: HashMap<String, f64> = HashMap::new();
            for (i, level) in col.iter().enumerate() {
                *sample_count.entry(level.clone()).or_insert(0.0) += weights[i];
            }

            // Compute ratio per observation: pop / sample.
            let mut obs_ratio = vec![1.0_f64; n];
            for (i, level) in col.iter().enumerate() {
                let pop = match population.get(level) {
                    Some(&p) if p > 0.0 => p,
                    _ => continue,
                };
                let samp = sample_count.get(level).copied().unwrap_or(0.0);
                if samp > 0.0 {
                    let ratio = pop / samp;
                    obs_ratio[i] = ratio;
                    let change = (ratio - 1.0).abs();
                    if change > max_change {
                        max_change = change;
                    }
                }
            }

            // Multiply weights by obs_ratio.
            for i in 0..n {
                weights[i] *= obs_ratio[i];
            }
        }
        if max_change < epsilon {
            // Converged.
            let _ = iter;
            break;
        }
    }

    // Build new SurveyDesign with updated weights (prob = 1/weight).
    let new_prob: Vec<f64> = weights.iter().map(|&w| 1.0 / w).collect();
    Ok(SurveyDesign {
        strata: design.strata.clone(),
        cluster: design.cluster.clone(),
        prob: new_prob,
        fpc: design.fpc.clone(),
        n_psu: design.n_psu.clone(),
        lonely_psu: design.lonely_psu,
        n_obs: n,
    })
}

/// Trim survey weights to `[lower, upper]`, redistributing excess weight.
///
/// Implements R's `trimWeights`: any weight outside the bounds is clamped,
/// and the trimmed excess is redistributed equally among untrimmed
/// observations. When `strict = true`, iterates until all weights are within
/// bounds (only weights not yet trimmed can absorb more).
///
/// Returns a new [`SurveyDesign`] with updated probabilities.
pub fn trim_weights(
    design: &SurveyDesign,
    upper: f64,
    lower: f64,
    strict: bool,
) -> Result<SurveyDesign> {
    let n = design.n_obs;
    let mut pw = design.weights();

    // If nothing is outside bounds, return unchanged.
    let outside: Vec<bool> = pw.iter().map(|&w| w < lower || w > upper).collect();
    if !outside.iter().any(|&b| b) {
        return Ok(design.clone());
    }

    let mut has_trimmed = vec![false; n];
    loop {
        let outside: Vec<bool> = pw.iter().map(|&w| w < lower || w > upper).collect();
        if !outside.iter().any(|&b| b) {
            break;
        }
        // Clamp.
        let pw_new: Vec<f64> = pw.iter().map(|&w| w.max(lower).min(upper)).collect();
        // Excess removed.
        let trimmings: Vec<f64> = pw.iter().zip(&pw_new).map(|(o, nw)| o - nw).collect();
        let total_trim: f64 = trimmings.iter().sum();
        // Can absorb: not outside, not already trimmed.
        let can_trim: Vec<bool> = outside
            .iter()
            .zip(&has_trimmed)
            .map(|(o, h)| !o && !h)
            .collect();
        let n_can = can_trim.iter().filter(|&&b| b).count();
        if n_can == 0 {
            break; // trimming failed — nothing left to redistribute to.
        }
        // Redistribute.
        let mut out = pw_new.clone();
        for i in 0..n {
            if can_trim[i] {
                out[i] += total_trim / n_can as f64;
            }
        }
        // Update state.
        has_trimmed = outside
            .iter()
            .zip(&has_trimmed)
            .map(|(o, h)| *o || *h)
            .collect();
        pw = out;
        if !strict {
            break;
        }
        // Loop back; the `outside` check at the top handles termination.
    }

    let new_prob: Vec<f64> = pw.iter().map(|&w| 1.0 / w).collect();
    Ok(SurveyDesign {
        strata: design.strata.clone(),
        cluster: design.cluster.clone(),
        prob: new_prob,
        fpc: design.fpc.clone(),
        n_psu: design.n_psu.clone(),
        lonely_psu: design.lonely_psu,
        n_obs: n,
    })
}

/// Linear (generalised regression) calibration.
///
/// Adjusts sampling weights so that weighted totals of auxiliary variables
/// match given population totals. The g-weight is
///   g_i = 1 + x_i' λ,  λ = (X'WX)^{-1} (T − X'W·1),
/// and the new weight is `w_i · g_i`.
///
/// # Arguments
/// - `design`: original survey design.
/// - `aux`: auxiliary variable values (n × p; `aux[j]` is a length-n column).
/// - `totals`: population totals for each auxiliary column (length p).
pub fn calibrate_linear(
    design: &SurveyDesign,
    aux: &[Vec<f64>],
    totals: &[f64],
) -> Result<SurveyDesign> {
    let n = design.n_obs;
    let p = aux.len();
    if p == 0 {
        return Err(SurveyError::InvalidInput("no auxiliary variables".into()));
    }
    if totals.len() != p {
        return Err(SurveyError::LengthMismatch {
            context: "totals vs aux".into(),
            a: totals.len(),
            b: p,
        });
    }
    for a in aux {
        if a.len() != n {
            return Err(SurveyError::LengthMismatch {
                context: "aux col vs design".into(),
                a: a.len(),
                b: n,
            });
        }
    }
    let w = design.weights();

    // X'WX (p×p) and X'W (sample totals, length p).
    let mut xtwx = vec![vec![0.0_f64; p]; p];
    let mut sample_totals = vec![0.0_f64; p];
    for i in 0..n {
        for a in 0..p {
            sample_totals[a] += aux[a][i] * w[i];
            for b in 0..p {
                xtwx[a][b] += aux[a][i] * w[i] * aux[b][i];
            }
        }
    }

    // λ = (X'WX)^{-1} (T - X'W·1)
    let rhs: Vec<f64> = (0..p).map(|a| totals[a] - sample_totals[a]).collect();
    let lambda = solve_linear(&xtwx, &rhs)?;

    // g_i = 1 + x_i' λ
    let mut g = vec![1.0_f64; n];
    for i in 0..n {
        for a in 0..p {
            g[i] += aux[a][i] * lambda[a];
        }
    }

    // new weight = w * g
    let new_prob: Vec<f64> = (0..n).map(|i| 1.0 / (w[i] * g[i])).collect();
    Ok(SurveyDesign {
        strata: design.strata.clone(),
        cluster: design.cluster.clone(),
        prob: new_prob,
        fpc: design.fpc.clone(),
        n_psu: design.n_psu.clone(),
        lonely_psu: design.lonely_psu,
        n_obs: n,
    })
}

/// Solve a symmetric positive-definite linear system A x = b.
fn solve_linear(a: &[Vec<f64>], b: &[f64]) -> Result<Vec<f64>> {
    use faer::linalg::solvers::{DenseSolveCore, Llt, Solve};
    let n = a.len();
    let a_m = faer::Mat::from_fn(n, n, |i, j| a[i][j]);
    let b_m = faer::Mat::from_fn(n, 1, |i, _| b[i]);
    let llt = Llt::new(a_m.as_ref(), faer::Side::Lower)
        .ok()
        .ok_or_else(|| SurveyError::InvalidInput("singular matrix in calibrate".into()))?;
    let x = llt.solve(&b_m);
    Ok((0..n).map(|i| x[(i, 0)]).collect())
}

/// Direct standardization (NCHS method).
///
/// Post-stratifies a design so the `by` variable matches `population`
/// proportions, applied within each level of `over`. The target cell count
/// for `(by, over)` is `population_prop[by] * freemargin[over]`, where
/// `freemargin[over]` is the weighted sample total in each `over` cell.
///
/// Returns a new design with updated weights.
pub fn svy_standardize(
    design: &SurveyDesign,
    by: &[String],
    over: &[String],
    population_prop: &HashMap<String, f64>,
) -> Result<SurveyDesign> {
    let n = design.n_obs;
    if by.len() != n || over.len() != n {
        return Err(SurveyError::LengthMismatch {
            context: "by/over vs design".into(),
            a: by.len(),
            b: n,
        });
    }
    let weights = design.weights();

    // Free margins: total weight per `over` cell.
    let mut free_margin: HashMap<String, f64> = HashMap::new();
    for i in 0..n {
        let over_level = &over[i];
        *free_margin.entry(over_level.clone()).or_insert(0.0) += weights[i];
    }

    // Population proportions sum should be ~1; normalize.
    let prop_sum: f64 = population_prop.values().sum();
    if prop_sum <= 0.0 {
        return Err(SurveyError::InvalidInput(
            "population proportions must have positive sum".into(),
        ));
    }

    // Target cell count for (by, over) = prop[by] * free_margin[over].
    let mut target: HashMap<(String, String), f64> = HashMap::new();
    for (over_level, free) in &free_margin {
        for (by_level, prop) in population_prop {
            target.insert(
                (by_level.clone(), over_level.clone()),
                (prop / prop_sum) * free,
            );
        }
    }

    // Sample cell counts.
    let mut sample_count: HashMap<(String, String), f64> = HashMap::new();
    for i in 0..n {
        let key = (by[i].clone(), over[i].clone());
        *sample_count.entry(key).or_insert(0.0) += weights[i];
    }

    // Ratio per observation.
    let mut reweight = vec![1.0_f64; n];
    for i in 0..n {
        let key = (by[i].clone(), over[i].clone());
        if let (Some(&t), Some(&s)) = (target.get(&key), sample_count.get(&key)) {
            if s > 0.0 {
                reweight[i] = t / s;
            }
        }
    }

    let mut new_prob = design.prob.clone();
    for i in 0..n {
        if (reweight[i] - 1.0).abs() > 1e-12 {
            new_prob[i] = design.prob[i] / reweight[i];
        }
    }

    Ok(SurveyDesign {
        strata: design.strata.clone(),
        cluster: design.cluster.clone(),
        prob: new_prob,
        fpc: design.fpc.clone(),
        n_psu: design.n_psu.clone(),
        lonely_psu: design.lonely_psu,
        n_obs: n,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::design::SurveyDesignBuilder;

    const FPC_X_TO_TEST: [f64; 8] = [2.8, 4.1, 6.8, 6.8, 9.2, 3.7, 6.6, 4.2];

    fn fpc_design() -> SurveyDesign {
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
            .build()
            .unwrap()
    }

    #[test]
    fn post_stratify_uniform_keeps_weights() {
        // If pop counts == sample counts, weights should be unchanged.
        let d = fpc_design();
        let strata = d.strata.clone();
        let mut pop = HashMap::new();
        pop.insert("1".into(), 15.0); // matches sample
        pop.insert("2".into(), 12.0); // matches sample
        let d2 = post_stratify(&d, &strata, &pop, false).unwrap();
        for i in 0..d.n_obs {
            assert!(
                (d2.weights()[i] - d.weights()[i]).abs() < 1e-9,
                "weight[{}] changed: {} -> {}",
                i,
                d.weights()[i],
                d2.weights()[i]
            );
        }
    }

    #[test]
    fn post_stratify_doubles_stratum_2() {
        // Set pop count for stratum 2 to 24 (double of 12). Each weight
        // in stratum 2 should double.
        let d = fpc_design();
        let strata = d.strata.clone();
        let mut pop = HashMap::new();
        pop.insert("1".into(), 15.0);
        pop.insert("2".into(), 24.0);
        let d2 = post_stratify(&d, &strata, &pop, false).unwrap();
        let w2 = d2.weights();
        for i in 0..5 {
            // Stratum 1: unchanged
            assert!((w2[i] - 3.0).abs() < 1e-9);
        }
        for i in 5..8 {
            // Stratum 2: doubled from 4 to 8
            assert!((w2[i] - 8.0).abs() < 1e-9, "w[{i}]={}", w2[i]);
        }
    }

    #[test]
    fn trim_weights_clamps_and_redistributes() {
        // Weights [3,3,3,3,3, 4,4,4], trim upper to 3.5.
        // Stratum 2 weights (4.0) exceed 3.5 → clamped to 3.5.
        // Total excess = 3*(4.0-3.5) = 1.5. Redistributed among 5 untrimmed obs.
        // Each gets 1.5/5 = 0.3. New: stratum1 = 3.3, stratum2 = 3.5.
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
        let d2 = trim_weights(&d, 3.5, f64::NEG_INFINITY, false).unwrap();
        let w = d2.weights();
        for i in 0..5 {
            assert!((w[i] - 3.3).abs() < 1e-9, "w[{i}]={}", w[i]);
        }
        for i in 5..8 {
            assert!((w[i] - 3.5).abs() < 1e-9, "w[{i}]={}", w[i]);
        }
    }

    #[test]
    fn calibrate_linear_matches_totals() {
        // Calibrate fpc data's `x` to a target total. Verify the calibrated
        // weighted total of x matches the target.
        let d = fpc_design();
        let x = FPC_X_TO_TEST.to_vec();
        let target = 200.0;
        let d2 = calibrate_linear(&d, std::slice::from_ref(&x), &[target]).unwrap();
        let w2 = d2.weights();
        let calibrated_total: f64 = x.iter().zip(&w2).map(|(&xi, &wi)| xi * wi).sum();
        assert!(
            (calibrated_total - target).abs() < 1e-6,
            "calibrated total: {calibrated_total} (target {target})"
        );
    }

    #[test]
    fn standardize_equal_props_is_noop() {
        // If population props equal the observed by-marginals within each
        // over cell, weights should be unchanged.
        let d = fpc_design();
        let by = d.strata.clone(); // use stratum as `by`
        let over = vec!["g".to_string(); 8]; // single over cell
        // Observed: by=1 weight 15, by=2 weight 12 → props 15/27, 12/27.
        let mut pop = HashMap::new();
        pop.insert("1".into(), 15.0 / 27.0);
        pop.insert("2".into(), 12.0 / 27.0);
        let d2 = svy_standardize(&d, &by, &over, &pop).unwrap();
        for i in 0..8 {
            assert!(
                (d2.weights()[i] - d.weights()[i]).abs() < 1e-9,
                "w[{}]: {}",
                i,
                d2.weights()[i]
            );
        }
    }

    #[test]
    fn standardize_changes_by_proportions() {
        let d = fpc_design();
        let by = d.strata.clone();
        let over = vec!["g".to_string(); 8];
        // Target: by=1 → 50%, by=2 → 50%. Observed by=1=15/27, by=2=12/27.
        let mut pop = HashMap::new();
        pop.insert("1".into(), 0.5);
        pop.insert("2".into(), 0.5);
        let d2 = svy_standardize(&d, &by, &over, &pop).unwrap();
        let w = d2.weights();
        // After standardization, weighted proportion of by=1 should be 0.5.
        let w1: f64 = w[0..5].iter().sum();
        let total: f64 = w.iter().sum();
        assert!((w1 / total - 0.5).abs() < 1e-6, "by=1 prop: {}", w1 / total);
    }

    #[test]
    fn trim_weights_noop_when_in_bounds() {
        let d = SurveyDesignBuilder::new()
            .strata(vec!["1".into(), "1".into()])
            .cluster(vec!["a".into(), "b".into()])
            .weights(vec![3.0, 3.0])
            .build()
            .unwrap();
        let d2 = trim_weights(&d, 10.0, 1.0, false).unwrap();
        assert!((d2.weights()[0] - 3.0).abs() < 1e-12);
        assert!((d2.weights()[1] - 3.0).abs() < 1e-12);
    }

    #[test]
    fn rake_matches_poststratify_for_one_var() {
        // Rake on a single margin is equivalent to post_stratify.
        let d = fpc_design();
        let mut pop = HashMap::new();
        pop.insert("1".into(), 20.0);
        pop.insert("2".into(), 16.0);
        let margins = vec![(d.strata.clone(), pop)];
        let d2 = rake(&d, &margins, 100, 1e-9).unwrap();
        let w2 = d2.weights();

        // After raking: sample count stratum 1 = 5*3 = 15 → ratio = 20/15 = 4/3.
        //              sample count stratum 2 = 3*4 = 12 → ratio = 16/12 = 4/3.
        // All weights should be multiplied by 4/3.
        for i in 0..5 {
            assert!((w2[i] - 4.0).abs() < 1e-9, "w[{}]={}", i, w2[i]);
        }
        for i in 5..8 {
            assert!((w2[i] - 16.0 / 3.0).abs() < 1e-9, "w[{}]={}", i, w2[i]);
        }
    }
}
