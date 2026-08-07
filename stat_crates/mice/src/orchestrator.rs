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

use rand::rngs::StdRng;
use rand::Rng;
use rand::SeedableRng;
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
    if levels.len() <= 2 {
        "logreg"
    } else {
        "pmm"
    }
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
        }
    }
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
                let meth = methods.get(i).cloned().unwrap_or_else(|| default_method_for(&data[c]).to_string());
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

    // Initialise imputations.
    let m = config.m;
    let mut imp = ImpList::default();
    imp.m = m;
    for col in &visit_sequence {
        imp.init_column(col, nmis[col], m);
    }
    // For each imputation i, fill imp[col][*, i] with a sample draw.
    let mut rng = match config.seed {
        Some(s) => StdRng::seed_from_u64(s),
        None => StdRng::from_entropy(),
    };
    for col in &visit_sequence {
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
    for col in &column_order {
        let r = &where_missing[col];
        if let Some(v) = current.get_mut(col) {
            for (r_idx, &ro) in r.iter().enumerate() {
                if !ro {
                    v[r_idx] = imp.get(col, 0, r_idx).unwrap_or(0.0);
                }
            }
        }
    }

    for _k in 0..config.maxit {
        for i in 0..m {
            // Step 1: pull the i-th draw into `current`.
            for col in &visit_sequence {
                let r = &where_missing[col];
                if let Some(v) = current.get_mut(col) {
                    for (r_idx, &ro) in r.iter().enumerate() {
                        if !ro {
                            v[r_idx] = imp.get(col, i, r_idx).unwrap_or(0.0);
                        }
                    }
                }
            }
            // Step 2: impute each column in visit order.
            for col in &visit_sequence {
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
                    "norm" => impute_norm(
                        &current[col],
                        r,
                        &x_rows,
                        Some(&wy),
                        config.ridge,
                        &mut rng,
                    )?,
                    "mean" => impute_mean(&current[col], r, Some(&wy), &mut rng),
                    "logreg" => impute_logreg(&current[col], r, &x_rows, Some(&wy), &mut rng)?,
                    "sample" => impute_sample(&current[col], r, Some(&wy), &mut rng),
                    "" => continue,
                    other => {
                        return Err(MiceError::UnknownMethod(other.to_string()));
                    }
                };

                // Update `imp[col][*, i]` and `current[col]`.
                let mut draw_idx = 0usize;
                for (idx, &w) in wy.iter().enumerate() {
                    if w {
                        let val = new_draws[draw_idx];
                        imp.set(col, i, idx, val);
                        if let Some(v) = current.get_mut(col) {
                            v[idx] = val;
                        }
                        draw_idx += 1;
                    }
                }
            }
        }
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

// silence unused-import clippy on Rng (kept in scope for future use).
#[allow(dead_code)]
fn _rng_ref<R: Rng + ?Sized>() {}
