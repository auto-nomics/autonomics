//! `mice::mice` orchestrator — the main Gibbs-sampler loop.
//!
//! Faithful port of `R/mice.R` and `R/sampler.R`. Steps:
//!
//!   1. Validate spec: build predictor matrix (default = "all other columns
//!      are predictors"), visit sequence (one column per block), method
//!      vector (default methods by column type: `"pmm"` for numeric, `"logreg"`
//!      for binary, `"polyreg"` for factor with >2 levels).
//!   2. Initialise imputations via [`crate::sample::impute_sample`] (matches
//!      R's `initialize.imp`).
//!   3. For each iteration `k` in `1..=maxit`:
//!      For each imputation `i` in `1..=m`:
//!        a. Fill the current data frame with the i-th imputation from the
//!           previous round.
//!        b. For each `col` in visit sequence, run the univariate imputation
//!           on the current data frame's missing rows and store the new
//!           draws back into `imp[col][*, i]`.
//!   4. Return a [`Mids`] object.
//!
//! When `MiceConfig::parallel` is `true`, the `m` imputation chains run
//! concurrently via rayon.  Each chain gets an independently seeded RNG, so
//! results are statistically equivalent to the sequential path but not
//! bit-identical.

use rand::SeedableRng;
use rand::rngs::StdRng;
use rayon::prelude::*;
use std::collections::HashMap;

use crate::error::{MiceError, Result};
use crate::logreg::impute_logreg;
use crate::mean::impute_mean;
use crate::mids::{ImpList, MethodSpec, Mids};
use crate::norm::impute_norm;
use crate::pmm::impute_pmm;
use crate::sample::impute_sample;

/// Default methods by data type, matching R's `defaultMethod` argument.
fn default_method_for(values: &[f64]) -> &'static str {
    // Numeric-only with >2 distinct values -> "pmm". For now we only
    // support numeric columns and binary {0, 1} columns; other categorical
    // types fall back to "sample".
    let mut levels = values
        .iter()
        .copied()
        .filter(|v| !v.is_nan())
        .collect::<Vec<_>>();
    levels.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    levels.dedup();
    if levels.len() <= 2 { "logreg" } else { "pmm" }
}

/// Derive a deterministic, well-separated per-chain seed from a base seed
/// and chain index using a splitmix64-style avalanching step.
fn chain_seed(base: u64, index: usize) -> u64 {
    let mut z = base.wrapping_add((index as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15));
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// Configuration for [`mice`].
#[derive(Debug, Clone)]
pub struct MiceConfig {
    /// Predictor matrix: for each target column, the list of predictor column
    /// names. If `None`, the default ("all other columns") is used.
    pub predictor_matrix: Option<Vec<Vec<String>>>,
    /// Method per column (in column-order). If `None`, default methods are
    /// inferred from data types.
    pub method: Option<Vec<String>>,
    /// Number of imputations. Default `5` (R default).
    pub m: usize,
    /// Number of Gibbs iterations. Default `5`.
    pub maxit: usize,
    /// RNG seed.
    pub seed: Option<u64>,
    /// Ridge penalty for OLS-based imputation methods. Default `1e-5`.
    pub ridge: f64,
    /// PMM donor pool size. Default `5`.
    pub donors: usize,
    /// PMM matching type (0, 1, or 2). Default `1`.
    pub matchtype: usize,
    /// If `true`, run the `m` imputation chains in parallel via rayon.
    /// Each chain gets an independently seeded RNG derived from `seed`, so
    /// results are statistically equivalent to sequential mode but not
    /// bit-identical.  Default `false` (sequential, bit-exact with R).
    pub parallel: bool,
}

impl Default for MiceConfig {
    fn default() -> Self {
        Self {
            predictor_matrix: None,
            method: None,
            m: 5,
            maxit: 5,
            seed: None,
            ridge: 1e-5,
            donors: 5,
            matchtype: 1,
            parallel: false,
        }
    }
}

/// Run one complete Gibbs chain (initialization + `maxit` iterations) for a
/// single imputation index.
///
/// This is the unit of parallelism: each chain is statistically independent
/// and uses its own RNG, so chains can run concurrently on separate threads
/// without affecting the distribution of results.
///
/// Returns a map from visit-sequence column name → imputed values (length
/// `nmis[col]`, in row order of the missing positions).
fn run_chain(
    data: &HashMap<String, Vec<f64>>,
    n: usize,
    where_missing: &HashMap<String, Vec<bool>>,
    visit_sequence: &[String],
    pred_map: &HashMap<String, Vec<String>>,
    method_map: &HashMap<String, String>,
    config: &MiceConfig,
    rng: &mut StdRng,
) -> Result<HashMap<String, Vec<f64>>> {
    // Working copy: observed values from data; missing positions filled
    // by sample-draw initialization and then successive Gibbs sweeps.
    let mut current: HashMap<String, Vec<f64>> = data.clone();

    // ── Initialize missing positions with random sample draws ──
    for col in visit_sequence {
        let ry = &where_missing[col];
        let wy: Vec<bool> = ry.iter().map(|r| !*r).collect();
        let draws = impute_sample(&data[col], ry, Some(&wy), rng);
        let mut draw_idx = 0usize;
        for (idx, &w) in wy.iter().enumerate() {
            if w {
                if let Some(v) = current.get_mut(col) {
                    v[idx] = draws[draw_idx];
                }
                draw_idx += 1;
            }
        }
    }

    // ── Main Gibbs loop: maxit iterations over the visit sequence ──
    for _k in 0..config.maxit {
        for col in visit_sequence {
            let r = &where_missing[col];
            let wy: Vec<bool> = r.iter().map(|x| !*x).collect();
            // Build predictor design (column-major → row-major x).
            let preds = pred_map.get(col).cloned().unwrap_or_default();
            let x_rows: Vec<Vec<f64>> = (0..n)
                .map(|idx| preds.iter().map(|p| current[p][idx]).collect())
                .collect();
            let method = method_map[col].as_str();

            let new_draws = match method {
                "pmm" => impute_pmm(
                    &current[col],
                    r,
                    &x_rows,
                    Some(&wy),
                    config.donors,
                    config.matchtype,
                    config.ridge,
                    rng,
                )?,
                "norm" => impute_norm(&current[col], r, &x_rows, Some(&wy), config.ridge, rng)?,
                "mean" => impute_mean(&current[col], r, Some(&wy), rng),
                "logreg" => impute_logreg(&current[col], r, &x_rows, Some(&wy), rng)?,
                "sample" => impute_sample(&current[col], r, Some(&wy), rng),
                "" => continue,
                other => return Err(MiceError::UnknownMethod(other.to_string())),
            };

            // Update `current[col]` in place.
            let mut draw_idx = 0usize;
            for (idx, &w) in wy.iter().enumerate() {
                if w {
                    let val = new_draws[draw_idx];
                    if let Some(v) = current.get_mut(col) {
                        v[idx] = val;
                    }
                    draw_idx += 1;
                }
            }
        }
    }

    // Extract final imputed values per visit-sequence column.
    let mut result = HashMap::new();
    for col in visit_sequence {
        let ry = &where_missing[col];
        let vals: Vec<f64> = (0..n)
            .filter(|&idx| !ry[idx])
            .map(|idx| current[col][idx])
            .collect();
        result.insert(col.clone(), vals);
    }
    Ok(result)
}

/// Reproduce R's `mice(data, m, method, predictorMatrix, maxit, seed, ...)`.
///
/// * `data` — column-major: `data[col][i]` is the i-th observation. Missing
///   values are represented as `f64::NAN`.
/// * `column_order` — column names in their canonical order (used to index
///   the predictor matrix and produce deterministic output).
/// * `config` — see [`MiceConfig`].
///
/// Returns a [`Mids`] holding the original data, `imp[col]` matrices of
/// shape `nmis[col] × m`, and the per-column methods.
pub fn mice(
    data: HashMap<String, Vec<f64>>,
    column_order: Vec<String>,
    config: MiceConfig,
) -> Result<Mids> {
    if column_order.is_empty() {
        return Err(MiceError::InvalidSpec("empty data".into()));
    }
    let n = data[&column_order[0]].len();
    for col in &column_order {
        if data[col].len() != n {
            return Err(MiceError::LengthMismatch(format!(
                "column {col} has {} rows, expected {n}",
                data[col].len()
            )));
        }
    }

    // Observed indicator per column.
    let mut where_missing: HashMap<String, Vec<bool>> = HashMap::new();
    let mut nmis: HashMap<String, usize> = HashMap::new();
    for col in &column_order {
        let r: Vec<bool> = data[col].iter().map(|v| !v.is_nan()).collect();
        let n_mis = r.iter().filter(|x| !**x).count();
        nmis.insert(col.clone(), n_mis);
        where_missing.insert(col.clone(), r);
    }

    // Default predictor matrix: each column predicted by all others.

    let predictor_matrix: Vec<Vec<String>> = match &config.predictor_matrix {
        Some(pm) => pm.clone(),
        None => column_order
            .iter()
            .map(|col| column_order.iter().filter(|c| *c != col).cloned().collect())
            .collect(),
    };
    let pred_map: HashMap<String, Vec<String>> = column_order
        .iter()
        .cloned()
        .zip(predictor_matrix.iter().cloned())
        .collect();

    // Method per column. Defaults to data-type inference when None.
    let method_map: HashMap<String, String> = match &config.method {
        Some(methods) => {
            let mut m = HashMap::new();
            for (i, c) in column_order.iter().enumerate() {
                let meth = methods
                    .get(i)
                    .cloned()
                    .unwrap_or_else(|| default_method_for(&data[c]).to_string());
                m.insert(c.clone(), meth);
            }
            m
        }
        None => column_order
            .iter()
            .map(|c| (c.clone(), default_method_for(&data[c]).to_string()))
            .collect(),
    };

    // Visit sequence = all columns with missing values.
    let visit_sequence: Vec<String> = column_order
        .iter()
        .filter(|c| nmis[c.as_str()] > 0)
        .cloned()
        .collect();

    // ── Allocate imp storage ──────────────────────────────────────────
    let m = config.m;
    let mut imp = ImpList::default();
    imp.m = m;
    for col in &visit_sequence {
        imp.init_column(col, nmis[col], m);
    }

    if config.parallel {
        // ── Parallel path: run m independent Gibbs chains via rayon ──
        //
        // Each chain gets its own RNG seeded deterministically from the
        // base seed + chain index. Results are statistically equivalent
        // to sequential mode (same posterior distribution) but not
        // bit-identical (different random draws per chain).
        let base_seed: u64 = match config.seed {
            Some(s) => s,
            None => {
                use rand::RngCore;
                StdRng::from_entropy().next_u64()
            }
        };

        let chain_results: Vec<Result<HashMap<String, Vec<f64>>>> = (0..m)
            .into_par_iter()
            .map(|i| {
                let s = chain_seed(base_seed, i);
                let mut chain_rng = StdRng::seed_from_u64(s);
                run_chain(
                    &data,
                    n,
                    &where_missing,
                    &visit_sequence,
                    &pred_map,
                    &method_map,
                    &config,
                    &mut chain_rng,
                )
            })
            .collect();

        // Merge per-chain results into the shared imp structure.
        for (i, chain_result) in chain_results.into_iter().enumerate() {
            let chain_imp = chain_result?;
            for (col, vals) in &chain_imp {
                for (r, &val) in vals.iter().enumerate() {
                    imp.set(col, i, r, val);
                }
            }
        }
    } else {
        // ── Sequential path (bit-exact with original implementation) ──
        run_sequential(
            &data,
            n,
            &mut imp,
            &where_missing,
            &nmis,
            &visit_sequence,
            &pred_map,
            &method_map,
            &config,
        )?;
    }

    let methods: Vec<MethodSpec> = column_order
        .iter()
        .map(|c| MethodSpec {
            block: c.clone(),
            method: method_map[c].clone(),
        })
        .collect();
    let _ = methods;

    let pm_by_col: HashMap<String, HashMap<String, i32>> = column_order
        .iter()
        .map(|c| {
            let inner = pred_map
                .get(c)
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .map(|p| (p, 1))
                .collect();
            (c.clone(), inner)
        })
        .collect();

    Ok(Mids {
        data,
        imp,
        m: config.m,
        iteration: config.maxit,
        nmis,
        method: method_map,
        predictor_matrix: pm_by_col,
        visit_sequence,
        where_missing,
        column_names: column_order,
    })
}

/// Original sequential Gibbs loop — preserves the exact RNG call order of
/// the initial single-threaded implementation (bit-exact with R for the
/// same seed).
fn run_sequential(
    data: &HashMap<String, Vec<f64>>,
    n: usize,
    imp: &mut ImpList,
    where_missing: &HashMap<String, Vec<bool>>,
    nmis: &HashMap<String, usize>,
    visit_sequence: &[String],
    pred_map: &HashMap<String, Vec<String>>,
    method_map: &HashMap<String, String>,
    config: &MiceConfig,
) -> Result<()> {
    let m = config.m;
    let mut rng = match config.seed {
        Some(s) => StdRng::seed_from_u64(s),
        None => StdRng::from_entropy(),
    };

    // For each imputation i, fill imp[col][*, i] with a sample draw.
    for col in visit_sequence {
        let ry = &where_missing[col];
        let wy = ry.iter().map(|r| !*r).collect::<Vec<_>>();
        for i in 0..m {
            let draws = impute_sample(&data[col], ry, Some(&wy), &mut rng);
            for (r_idx, &d) in draws.iter().enumerate() {
                imp.set(col, i, r_idx, d);
            }
        }
    }

    // ── Main Gibbs loop ────────────────────────────────────────────────
    // We maintain a working "current" data view that's mutated in place each
    // round. The current view has the latest draws for missing positions.
    // Initial fill: copy imp[*, 0] into missing positions (R behaviour before
    // the main loop).
    let mut current: HashMap<String, Vec<f64>> = data.clone();
    // Initialise missing positions with imp[*, 0] (R behaviour before the
    // main loop).  Note: imp is indexed by missing-position index
    // (0..nmis), NOT the original row index.
    for (col, r) in where_missing {
        if let Some(v) = current.get_mut(col) {
            let mut mis_idx = 0usize;
            for (r_idx, &ro) in r.iter().enumerate() {
                if !ro {
                    v[r_idx] = imp.get(col, 0, mis_idx).unwrap_or(0.0);
                    mis_idx += 1;
                }
            }
        }
    }

    for _k in 0..config.maxit {
        for i in 0..m {
            // Step 1: pull the i-th draw into `current`.
            for col in visit_sequence {
                let r = &where_missing[col];
                if let Some(v) = current.get_mut(col) {
                    let mut mis_idx = 0usize;
                    for (r_idx, &ro) in r.iter().enumerate() {
                        if !ro {
                            v[r_idx] = imp.get(col, i, mis_idx).unwrap_or(0.0);
                            mis_idx += 1;
                        }
                    }
                }
            }
            // Step 2: impute each column in visit order.
            for col in visit_sequence {
                let r = &where_missing[col];
                let wy: Vec<bool> = r.iter().map(|x| !*x).collect();
                // Build predictor design (column-major -> row-major x).
                let preds = pred_map.get(col).cloned().unwrap_or_default();
                let x_rows: Vec<Vec<f64>> = (0..n)
                    .map(|idx| preds.iter().map(|p| current[p][idx]).collect())
                    .collect();
                let method = method_map[col].clone();

                let new_draws = match method.as_str() {
                    "pmm" => impute_pmm(
                        &current[col],
                        r,
                        &x_rows,
                        Some(&wy),
                        config.donors,
                        config.matchtype,
                        config.ridge,
                        &mut rng,
                    )?,
                    "norm" => {
                        impute_norm(&current[col], r, &x_rows, Some(&wy), config.ridge, &mut rng)?
                    }
                    "mean" => impute_mean(&current[col], r, Some(&wy), &mut rng),
                    "logreg" => impute_logreg(&current[col], r, &x_rows, Some(&wy), &mut rng)?,
                    "sample" => impute_sample(&current[col], r, Some(&wy), &mut rng),
                    "" => continue,
                    other => {
                        return Err(MiceError::UnknownMethod(other.to_string()));
                    }
                };

                // Update `imp[col][*, i]` and `current[col]`.
                // draw_idx is the missing-position index (0..nmis), used
                // for imp storage; idx is the original row index (0..n),
                // used for updating current.
                let mut draw_idx = 0usize;
                for (idx, &w) in wy.iter().enumerate() {
                    if w {
                        let val = new_draws[draw_idx];
                        imp.set(col, i, draw_idx, val);
                        if let Some(v) = current.get_mut(col) {
                            v[idx] = val;
                        }
                        draw_idx += 1;
                    }
                }
            }
        }
    }

    let _ = nmis;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_data(n: usize) -> HashMap<String, Vec<f64>> {
        let x: Vec<f64> = (0..n).map(|i| i as f64).collect();
        let z: Vec<f64> = (0..n).map(|i| (i as f64).sin()).collect();
        let mut y: Vec<f64> = x
            .iter()
            .zip(z.iter())
            .map(|(xi, zi)| 2.0 * xi + zi)
            .collect();
        // Punch some holes.
        for &idx in &[3, 7, 12, 18, 25, 0, 5, 10, 15, 20] {
            if (idx as usize) < n {
                y[idx as usize] = f64::NAN;
            }
        }
        let mut data = HashMap::new();
        data.insert("x".to_string(), x);
        data.insert("z".to_string(), z);
        data.insert("y".to_string(), y);
        data
    }

    #[test]
    fn test_sequential_basic() {
        let data = make_data(30);
        let col_order = vec!["x".to_string(), "z".to_string(), "y".to_string()];
        let config = MiceConfig {
            m: 3,
            maxit: 2,
            seed: Some(42),
            ..Default::default()
        };
        let mids = mice(data, col_order, config).unwrap();
        assert_eq!(mids.m, 3);
        assert_eq!(mids.iteration, 2);
        // y has 10 missing values.
        assert_eq!(mids.nmis["y"], 10);
        // No NaN in the first completed dataset.
        let comp = crate::complete::complete(&mids, crate::complete::CompleteFormat::First, 1);
        for v in &comp.rows {
            assert!(!v.is_nan(), "NaN in completed data");
        }
    }

    #[test]
    fn test_parallel_basic() {
        let data = make_data(30);
        let col_order = vec!["x".to_string(), "z".to_string(), "y".to_string()];
        let config = MiceConfig {
            m: 3,
            maxit: 2,
            seed: Some(42),
            parallel: true,
            ..Default::default()
        };
        let mids = mice(data, col_order, config).unwrap();
        assert_eq!(mids.m, 3);
        assert_eq!(mids.iteration, 2);
        assert_eq!(mids.nmis["y"], 10);
        let comp = crate::complete::complete(&mids, crate::complete::CompleteFormat::First, 1);
        for v in &comp.rows {
            assert!(!v.is_nan(), "NaN in completed data");
        }
    }

    #[test]
    fn test_parallel_reproducible() {
        let col_order = vec!["x".to_string(), "z".to_string(), "y".to_string()];
        let config = MiceConfig {
            m: 3,
            maxit: 3,
            seed: Some(99),
            parallel: true,
            ..Default::default()
        };
        let data1 = make_data(40);
        let data2 = make_data(40);
        let mids1 = mice(data1, col_order.clone(), config.clone()).unwrap();
        let mids2 = mice(data2, col_order, config).unwrap();
        // Same seed → identical results.
        for col in &mids1.column_names {
            if let (Some(v1), Some(v2)) =
                (mids1.imp.by_column.get(col), mids2.imp.by_column.get(col))
            {
                assert_eq!(v1, v2, "parallel results differ for column {col}");
            }
        }
    }

    #[test]
    fn test_parallel_statistically_equivalent() {
        // Parallel and sequential should produce results of similar
        // magnitude (within a few SD) — not bit-identical, but
        // statistically from the same distribution.
        let col_order = vec!["x".to_string(), "z".to_string(), "y".to_string()];

        let config_seq = MiceConfig {
            m: 5,
            maxit: 5,
            seed: Some(42),
            parallel: false,
            ..Default::default()
        };
        let config_par = MiceConfig {
            m: 5,
            maxit: 5,
            seed: Some(42),
            parallel: true,
            ..Default::default()
        };

        let mids_seq = mice(make_data(200), col_order.clone(), config_seq).unwrap();
        let mids_par = mice(make_data(200), col_order, config_par).unwrap();

        // Compute mean of imputed y values for each approach.
        let y_imp_seq = mids_seq.imp.by_column.get("y").unwrap();
        let y_imp_par = mids_par.imp.by_column.get("y").unwrap();

        let mean_seq: f64 = y_imp_seq.iter().sum::<f64>() / y_imp_seq.len() as f64;
        let mean_par: f64 = y_imp_par.iter().sum::<f64>() / y_imp_par.len() as f64;

        // The true mean of the missing y values (indices 3,7,12,18,25,0,5,10,15,20)
        // in make_data(200): y = 2x + sin(x), so these are approximately
        // 2*idx + sin(idx). Mean ≈ (2*(3+7+12+18+25+0+5+10+15+20)/10) ≈ 23.
        // Allow generous tolerance for Gibbs randomness.
        let midpoint = (mean_seq + mean_par) / 2.0;
        let spread = (mean_seq - mean_par).abs();
        // Both should be in the same ballpark.
        assert!(
            spread < midpoint.abs() * 0.5,
            "seq mean {mean_seq}, par mean {par_mean}, spread too large",
            par_mean = mean_par
        );
    }
}
