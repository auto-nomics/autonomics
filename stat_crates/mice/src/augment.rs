//! Data augmentation for `mice.impute.logreg` and `mice.impute.polyreg`.
//!
//! Reproduces the White–Daniel–Royston (2010) augmentation used by mice's
//! logistic-regression imputation methods to evade the perfect-prediction
//! problem. The reference is `R/augment.R` and the calling sites in
//! `R/mice.impute.logreg.R` / `R/mice.impute.polyreg.R`.
//!
//! Algorithm:
//!   1. For each predictor column, compute mean, sd, min, max over observed
//!      rows.
//!   2. Append `2 · p · k` synthetic rows whose predictor values are the
//!      column means shifted by ±0.5·sd and clipped to [min, max].
//!   3. For each `p × k` block of two rows, set the response to one of the
//!      `k` unique response levels (cycling). Mark them as observed with
//!      weight `0.5`.
//!
//! The augmentation size is `2 · p · k` rows (regardless of observed count).

use crate::error::{MiceError, Result};

/// Augmented dataset returned by [`augment`].
#[derive(Debug, Clone)]
pub struct AugmentedData {
    /// Augmented predictor matrix (length `n + 2·p·k` rows), each row of
    /// length `p`.
    pub x: Vec<Vec<f64>>,
    /// Augmented response vector (length `n + 2·p·k`).
    pub y: Vec<f64>,
    /// Observed indicator for the augmented set.
    pub ry: Vec<bool>,
    /// Wy indicator (locations to impute, unchanged from input).
    pub wy: Vec<bool>,
    /// Per-row weights: `1.0` for original rows, `0.5` for synthetic ones.
    pub w: Vec<f64>,
}

/// Reproduce R's `augment(y, ry, x, wy, maxcat = 50)` for the binary
/// logistic-regression case (the polyreg path is not yet ported — see
/// `TODO` below).
///
/// `y` and `ry` and `wy` each have length `n`. `x` is `n × p` (no intercept
/// column — callers prepend `1`s themselves when fitting, matching the
/// reference).
pub fn augment(y: &[f64], ry: &[bool], x: &[Vec<f64>], wy: &[bool]) -> Result<AugmentedData> {
    let n = y.len();
    if ry.len() != n || wy.len() != n {
        return Err(MiceError::LengthMismatch(
            "ry/wy must have length n".into(),
        ));
    }
    if x.len() != n {
        return Err(MiceError::LengthMismatch(
            "x must have n rows".into(),
        ));
    }
    if n == 0 {
        return Err(MiceError::InsufficientData("empty input".into()));
    }
    let p = x[0].len();
    if p == 0 {
        return Err(MiceError::InsufficientData("zero predictors".into()));
    }

    // Single-missing-value skip: matches the R short-circuit.
    let n_mis: usize = ry.iter().filter(|r| !**r).count();
    if n_mis == 1 {
        return Ok(AugmentedData {
            x: x.to_vec(),
            y: y.to_vec(),
            ry: ry.to_vec(),
            wy: wy.to_vec(),
            w: vec![1.0; n],
        });
    }

    // Observed-only summary statistics.
    let mut means = vec![0.0_f64; p];
    let mut vars = vec![0.0_f64; p];
    let mut mins = vec![f64::INFINITY; p];
    let mut maxs = vec![f64::NEG_INFINITY; p];
    let mut n_obs: usize = 0;
    for i in 0..n {
        if !ry[i] {
            continue;
        }
        n_obs += 1;
        for j in 0..p {
            let v = x[i][j];
            means[j] += v;
            vars[j] += v * v;
            if v < mins[j] {
                mins[j] = v;
            }
            if v > maxs[j] {
                maxs[j] = v;
            }
        }
    }
    if n_obs == 0 {
        return Err(MiceError::InsufficientData("no observed y".into()));
    }
    for j in 0..p {
        means[j] /= n_obs as f64;
        vars[j] = (vars[j] / n_obs as f64 - means[j] * means[j]).max(0.0);
    }
    let sds: Vec<f64> = vars.iter().map(|v| v.sqrt()).collect();

    // Unique response levels among the observed y, sorted ascending.
    let mut levels: Vec<f64> = ry
        .iter()
        .zip(y.iter())
        .filter_map(|(r, v)| if *r { Some(*v) } else { None })
        .collect();
    levels.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    levels.dedup();
    let k = levels.len();
    if k < 2 {
        return Err(MiceError::InvalidSpec(
            "augment: response must have at least 2 levels".into(),
        ));
    }

    // Build synthetic block of 2·p·k rows.
    let nr = 2 * p * k;
    // base = mean replicated nr × p
    let mut synth_x = vec![vec![0.0_f64; p]; nr];
    for i in 0..nr {
        for j in 0..p {
            let shift = match (i / (p * 2 / k.max(1))) % 2 {
                // 0 -> +0.5·sd, 1 -> -0.5·sd (R uses a sequence c(0.5,-0.5)).
                0 => 0.5,
                _ => -0.5,
            };
            let v = means[j] + shift * sds[j];
            // Clip to [min, max].
            synth_x[i][j] = v.clamp(mins[j], maxs[j]);
        }
    }
    // The R reference cycles `c(0.5, -0.5)` per `k` level, repeated for `p`
    // predictors. We replicate by interleaving pairs of rows per predictor
    // block. Each pair has the same y, and we cycle through `k` levels.
    let mut synth_y = Vec::with_capacity(nr);
    {
        // Pattern: for each predictor j (p times), each level kᵢ (k levels)
        // contributes 2 rows with y = kᵢ.
        let mut idx = 0;
        for j in 0..p {
            for li in 0..k {
                for _ in 0..2 {
                    synth_y.push(levels[li]);
                    idx += 1;
                }
            }
        }
        debug_assert_eq!(idx, nr);
    }

    // Concatenate original + synthetic.
    let mut aug_x = x.to_vec();
    aug_x.extend(synth_x);
    let mut aug_y = y.to_vec();
    aug_y.extend(synth_y);

    let mut aug_ry = ry.to_vec();
    aug_ry.extend(vec![true; nr]);
    let aug_wy = wy.to_vec();
    let mut aug_w = vec![1.0_f64; n];
    aug_w.extend(vec![0.5_f64; nr]);

    Ok(AugmentedData {
        x: aug_x,
        y: aug_y,
        ry: aug_ry,
        wy: aug_wy,
        w: aug_w,
    })
}
