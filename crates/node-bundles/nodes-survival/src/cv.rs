//! Shared leak-free cross-validation machinery for the chordoma-protocol
//! orchestration nodes ([`crate::nested_oof`], [`crate::fusion`]).
//!
//! Everything here is deterministic: fold assignment comes from a seeded
//! xorshift shuffle, `λ` selection never touches outer-fold test rows, and
//! the inner-CV criterion is the same IPCW Brier score ([`crrkit::brier`])
//! used for the locked endpoint — so model selection and evaluation share
//! one metric, as the SAP requires.

use cmprsk::{CrrInput, CrrRidgeFit, CrrRidgeOptions, TimeFunctions, crr_ridge};
use crrkit::brier::{BrierOptions, ipcw_brier};

/// Deterministic xorshift64* — dependency-free pseudo-randomness, the same
/// generator family the `cmprsk` test-suite uses.
pub(crate) struct XorShift(u64);

impl XorShift {
    pub(crate) fn new(seed: u64) -> Self {
        // A zero state would degenerate; fold it with a fixed constant.
        Self(seed ^ 0x2545F4914F6CDD1D | 1)
    }
    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545F4914F6CDD1D)
    }
    /// Fisher–Yates shuffle of a slice of indices.
    pub(crate) fn shuffle(&mut self, xs: &mut [usize]) {
        for i in (1..xs.len()).rev() {
            let j = (self.next_u64() % (i as u64 + 1)) as usize;
            xs.swap(i, j);
        }
    }
}

/// Assign stratified fold ids in `[0, n_folds)`: within each stratum the
/// member indices are deterministically shuffled (seeded), then dealt to
/// folds round-robin with a per-call `offset` so inner and outer splits of
/// the same subjects differ.
///
/// With `strata` all-zero this is a plain seeded shuffle split.
pub(crate) fn stratified_folds(
    strata: &[u8],
    n_folds: usize,
    seed: u64,
    offset: usize,
) -> Vec<u32> {
    let mut folds = vec![0u32; strata.len()];
    let levels: Vec<u8> = {
        use std::collections::BTreeSet;
        strata.iter().copied().collect::<BTreeSet<_>>().into_iter().collect()
    };
    for level in levels {
        let mut idxs: Vec<usize> = strata
            .iter()
            .enumerate()
            .filter(|&(_, &s)| s == level)
            .map(|(i, _)| i)
            .collect();
        XorShift::new(seed ^ (u64::from(level)).wrapping_mul(0x9E3779B97F4A7C15))
            .shuffle(&mut idxs);
        for (pos, &i) in idxs.iter().enumerate() {
            folds[i] = ((pos + offset) % n_folds) as u32;
        }
    }
    folds
}

/// Recode a `cmprsk`-style status column into the `{0, 1, 2}` contract of
/// [`crrkit`]: `failcode → 1` (event of interest), `cencode → 0`, any other
/// value `→ 2` (competing event).
pub(crate) fn recode_status(
    fstatus: &[f64],
    failcode: f64,
    cencode: f64,
) -> Result<Vec<u8>, String> {
    fstatus
        .iter()
        .map(|&s| {
            if (s - failcode).abs() < f64::EPSILON {
                Ok(1u8)
            } else if (s - cencode).abs() < f64::EPSILON {
                Ok(0u8)
            } else if s.is_finite() {
                Ok(2u8)
            } else {
                Err(format!("status value {s} is not finite"))
            }
        })
        .collect()
}

/// Stratification indicator for fold balance: `1` when the subject had the
/// cause-1 event by the horizon, else `0`. Event subjects are rare in the
/// chordoma cohort, so unstratified folds can leave a fold with no events.
pub(crate) fn event_by_horizon_strata(time: &[f64], status_code: &[u8], horizon: f64) -> Vec<u8> {
    time.iter()
        .zip(status_code.iter())
        .map(|(&t, &s)| if s == 1 && t <= horizon { 1u8 } else { 0u8 })
        .collect()
}

/// Cumulative incidence `F(t*; x)` of one row from a fitted ridge Fine–Gray
/// model: walk the Breslow baseline to the horizon and evaluate
/// `1 − exp(−Λ₀(t*) · exp(x·β + intercept))`. Times before the first failure
/// give `0`.
pub(crate) fn cif_at(fit: &CrrRidgeFit, row: &[f64], horizon: f64) -> Result<f64, String> {
    let eta = fit
        .linear_predictor(row)
        .map_err(|e| e.to_string())?
        .exp();
    let mut acc = 0.0_f64;
    for (&t, &j) in fit.uftime.iter().zip(fit.bfitj.iter()) {
        if t > horizon {
            break;
        }
        acc += j * eta;
    }
    Ok(1.0 - (-acc).exp())
}

/// Outcome of an inner-CV `λ` search.
#[derive(Debug, Clone)]
pub(crate) struct LambdaSelection {
    /// The selected `λ` (smallest mean inner-OOF IPCW Brier, ties to the
    /// smaller `λ`).
    pub best: f64,
    /// `(lambda, mean inner-OOF IPCW Brier)` per grid point; entries whose
    /// every inner fold failed carry `f64::INFINITY`.
    pub mean_brier: Vec<(f64, f64)>,
    /// How many `(fold, λ)` evaluations failed (collapsed censoring, no
    /// outcome information, non-finite fit) and were skipped.
    pub n_failed_evals: usize,
}

/// Select `λ` on the training portion of one outer fold by inner K-fold CV,
/// minimizing the mean inner-OOF IPCW Brier at `horizon`.
///
/// `cov1` is the full design matrix in `[clinical…, penalized…]` layout with
/// `n_clinical` leading unpenalized columns; `idx` selects the training rows.
/// The evaluation rows of an inner fold never enter the fit of the model that
/// predicts them, and outer test rows never enter this function at all.
pub(crate) fn select_lambda_inner(
    time: &[f64],
    status_code: &[u8],
    cov1: &[Vec<f64>],
    terms: &[String],
    n_clinical: usize,
    idx: &[usize],
    grid: &[f64],
    n_inner: usize,
    horizon: f64,
    seed: u64,
    offset: usize,
    gtol: f64,
    maxiter: usize,
) -> Result<LambdaSelection, String> {
    // Training rows of this outer fold, in the caller's row space.
    let t_train: Vec<f64> = idx.iter().map(|&i| time[i]).collect();
    let s_train: Vec<u8> = idx.iter().map(|&i| status_code[i]).collect();
    let x_train: Vec<Vec<f64>> = idx.iter().map(|&i| cov1[i].clone()).collect();
    let strata = event_by_horizon_strata(&t_train, &s_train, horizon);
    let inner = stratified_folds(&strata, n_inner, seed, offset);

    let brier_opts = BrierOptions::default();
    let unpenalized: Vec<usize> = (0..n_clinical).collect();
    let mut mean_brier = Vec::with_capacity(grid.len());
    let mut n_failed = 0usize;

    for &lambda in grid {
        let mut sum = 0.0_f64;
        let mut n_ok = 0usize;
        for fold in 0..n_inner {
            let val_pos: Vec<usize> = (0..idx.len()).filter(|&j| inner[j] as usize == fold).collect();
            let fit_pos: Vec<usize> = (0..idx.len()).filter(|&j| inner[j] as usize != fold).collect();
            if val_pos.is_empty() || fit_pos.is_empty() {
                n_failed += 1;
                continue;
            }
            let fit = crr_ridge(
                &CrrInput {
                    ftime: &subset(&t_train, &fit_pos),
                    fstatus: &subset(&s_train, &fit_pos)
                        .iter()
                        .map(|&s| f64::from(s))
                        .collect::<Vec<_>>(),
                    cov1: &subset(&x_train, &fit_pos),
                    cov1_names: terms,
                    cov2: &[],
                    cov2_names: &[],
                    tf: TimeFunctions::None,
                    cengroup: None,
                },
                &CrrRidgeOptions {
                    lambda,
                    unpenalized: unpenalized.clone(),
                    standardize: true,
                    gtol,
                    maxiter,
                    failcode: 1.0,
                    cencode: 0.0,
                },
            );
            let fit = match fit {
                Ok(f) => f,
                Err(_) => {
                    n_failed += 1;
                    continue;
                }
            };
            let val_t: Vec<f64> = subset(&t_train, &val_pos);
            let val_s: Vec<u8> = subset(&s_train, &val_pos);
            let mut probs = Vec::with_capacity(val_pos.len());
            let mut failed = false;
            for &j in &val_pos {
                match cif_at(&fit, &x_train[j], horizon) {
                    Ok(p) => probs.push(p),
                    Err(_) => {
                        failed = true;
                        break;
                    }
                }
            }
            if failed {
                n_failed += 1;
                continue;
            }
            match ipcw_brier(&val_t, &val_s, &probs, horizon, &brier_opts) {
                Ok(r) => {
                    sum += r.brier;
                    n_ok += 1;
                }
                Err(_) => {
                    n_failed += 1;
                }
            }
        }
        let mean = if n_ok == 0 {
            f64::INFINITY
        } else {
            sum / n_ok as f64
        };
        mean_brier.push((lambda, mean));
    }

    let best = mean_brier
        .iter()
        .filter(|(_, m)| m.is_finite())
        .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap().then(a.0.total_cmp(&b.0)))
        .map(|&(l, _)| l)
        .ok_or_else(|| {
            "every inner-CV evaluation failed (collapsed censoring or no outcome \
             information in all inner folds); check the horizon and event count"
                .to_string()
        })?;

    Ok(LambdaSelection {
        best,
        mean_brier,
        n_failed_evals: n_failed,
    })
}

/// Subset any row-indexed matrix/vector to the given positions.
fn subset<T: Clone>(xs: &[T], pos: &[usize]) -> Vec<T> {
    pos.iter().map(|&j| xs[j].clone()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folds_are_deterministic_balanced_and_offset_sensitive() {
        let strata = vec![0u8; 40];
        let a = stratified_folds(&strata, 5, 7, 0);
        let b = stratified_folds(&strata, 5, 7, 0);
        assert_eq!(a, b, "same seed must reproduce the split");
        assert_eq!(a.iter().filter(|&&f| f == 0).count(), 8);
        assert_eq!(a.iter().filter(|&&f| f == 4).count(), 8);
        let c = stratified_folds(&strata, 5, 7, 1);
        assert_ne!(a, c, "offset must change the assignment");
    }

    #[test]
    fn stratification_spreads_events_across_folds() {
        // 10 event subjects among 50: every fold must receive exactly 2.
        let mut strata = vec![0u8; 50];
        for s in strata.iter_mut().take(10) {
            *s = 1;
        }
        let folds = stratified_folds(&strata, 5, 3, 0);
        for f in 0..5u32 {
            let events = strata
                .iter()
                .zip(folds.iter())
                .filter(|&(&s, &fold)| s == 1 && fold == f)
                .count();
            assert_eq!(events, 2);
        }
    }

    #[test]
    fn recode_maps_failcode_cencode_and_competing() {
        let out = recode_status(&[1.0, 0.0, 2.0, 1.0], 1.0, 0.0).unwrap();
        assert_eq!(out, vec![1u8, 0, 2, 1]);
        assert!(recode_status(&[f64::NAN], 1.0, 0.0).is_err());
    }

    #[test]
    fn cif_at_steps_at_failure_times() {
        // Baseline with a single jump at t=10 of size ln 2 and β = 0.
        let fit = CrrRidgeFit {
            coef: vec![0.0],
            intercept: 0.0,
            coef_standardized: vec![0.0],
            center: vec![0.0],
            scale: vec![1.0],
            lambda: 0.0,
            unpenalized: vec![0],
            terms: vec!["x".into()],
            loglik: 0.0,
            objective_penalized: 0.0,
            var: vec![vec![0.0]],
            converged: true,
            n_iter: 1,
            uftime: vec![10.0, 20.0],
            bfitj: vec![2.0_f64.ln(), 2.0_f64.ln()],
            n: 10,
            n_missing: 0,
            n_events: 5,
            ncov1: 1,
        };
        assert!((cif_at(&fit, &[0.0], 5.0).unwrap() - 0.0).abs() < 1e-12);
        assert!((cif_at(&fit, &[0.0], 10.0).unwrap() - 0.5).abs() < 1e-12);
        // two jumps of ln2 → 1 - exp(-2 ln2) = 0.75
        assert!((cif_at(&fit, &[0.0], 30.0).unwrap() - 0.75).abs() < 1e-12);
    }
}
