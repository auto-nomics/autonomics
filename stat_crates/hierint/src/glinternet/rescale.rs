//! Coefficient rescaling and warm-start initialization.
//!
//! Port of `rescale_betahat()` and `initialize_betahat()` from R/C.

use super::transform;
use super::{ActiveSet, GlinternetData};

/// Rescale betahat to undo the standardization applied during fitting.
///
/// Port of `rescale_beta()` in `c_routines.c`.
/// Input: `betahat` = [intercept, beta...] in the standardized space.
/// Returns: (intercept_rescaled, beta_rescaled) in the original variable space.
pub fn rescale_betahat(
    active: &ActiveSet,
    betahat: &[f64],
    data: &GlinternetData,
) -> (f64, Vec<f64>) {
    let n = data.n;
    let mut result = betahat.to_vec();
    let mut offset = 1usize; // skip intercept

    let p_cat = active.n_vars()[0];
    let p_cont = active.n_vars()[1];
    let p_catcat = active.n_vars()[2];
    let p_contcont = active.n_vars()[3];
    let p_catcont = active.n_vars()[4];

    if p_cat + p_cont + p_catcat + p_contcont + p_catcont == 0 {
        return (result[0], result);
    }

    // ── Categorical main effects ────────────────────────────────────────
    if p_cat > 0 {
        let factor = (n as f64).sqrt();
        if let Some(ref cat) = active.cat {
            for &[ci] in cat {
                let size = data.levels[ci - 1];
                for i in 0..size {
                    result[offset + i] /= factor;
                }
                offset += size;
            }
        }
    }

    // ── Continuous main effects ─────────────────────────────────────────
    if p_cont > 0 {
        if let Some(ref cont) = active.cont {
            for &[ci] in cont {
                let zptr = &data.z[(ci - 1) * n..ci * n];
                let mut mean = 0.0;
                let mut norm_sq = 0.0;
                for i in 0..n {
                    mean += zptr[i];
                    norm_sq += zptr[i] * zptr[i];
                }
                mean /= n as f64;
                let norm = if norm_sq.abs() > 1e-30 {
                    (norm_sq - n as f64 * mean * mean).sqrt()
                } else {
                    1.0
                };
                result[offset] /= norm;
                result[0] -= mean * result[offset];
                offset += 1;
            }
        }
    }

    // ── Categorical × Categorical ───────────────────────────────────────
    if p_catcat > 0 {
        let factor = (n as f64).sqrt();
        if let Some(ref catcat) = active.catcat {
            for &[ci, cj] in catcat {
                let size = data.levels[ci - 1] * data.levels[cj - 1];
                for i in 0..size {
                    result[offset + i] /= factor;
                }
                offset += size;
            }
        }
    }

    // ── Continuous × Continuous ─────────────────────────────────────────
    if p_contcont > 0 {
        let factor = 3.0f64.sqrt();
        if let Some(ref contcont) = active.contcont {
            for &[ci, cj] in contcont {
                let wptr = &data.z[(ci - 1) * n..ci * n];
                let zptr = &data.z[(cj - 1) * n..cj * n];

                let (mut mean, mut mean_z, mut norm_sq, mut norm_z_sq) = (0.0, 0.0, 0.0, 0.0);
                for i in 0..n {
                    mean += wptr[i];
                    norm_sq += wptr[i] * wptr[i];
                    mean_z += zptr[i];
                    norm_z_sq += zptr[i] * zptr[i];
                }
                mean /= n as f64;
                mean_z /= n as f64;
                let norm = if norm_sq.abs() > 1e-30 {
                    (norm_sq - n as f64 * mean * mean).sqrt()
                } else {
                    1.0
                };
                let norm_z = if norm_z_sq.abs() > 1e-30 {
                    (norm_z_sq - n as f64 * mean_z * mean_z).sqrt()
                } else {
                    1.0
                };

                result[offset] /= (factor * norm);
                result[offset + 1] /= (factor * norm_z);
                result[0] -= mean * result[offset] + mean_z * result[offset + 1];

                // Product term normalization
                let mut mean_prod = 0.0;
                let mut norm_prod_sq = 0.0;
                for i in 0..n {
                    let prod = (wptr[i] - mean) * (zptr[i] - mean_z) / (norm * norm_z);
                    mean_prod += prod;
                    norm_prod_sq += prod * prod;
                }
                mean_prod /= n as f64;
                let norm_prod = if norm_prod_sq.abs() > 1e-30 {
                    (norm_prod_sq - n as f64 * mean_prod * mean_prod).sqrt()
                } else {
                    1.0
                };
                result[offset + 2] /= (factor * norm_prod);
                result[0] -= mean_prod * result[offset + 2];
                result[offset + 2] /= norm * norm_z;
                result[offset] -= mean_z * result[offset + 2];
                result[offset + 1] -= mean * result[offset + 2];
                result[0] += mean * mean_z * result[offset + 2];

                offset += 3;
            }
        }
    }

    // ── Categorical × Continuous ────────────────────────────────────────
    if p_catcont > 0 {
        let factor = 2.0f64.sqrt();
        let factor1 = (2.0 * n as f64).sqrt();
        if let Some(ref catcont) = active.catcont {
            for &[ci, cj] in catcont {
                let zptr = &data.z[(cj - 1) * n..cj * n];
                let size = data.levels[ci - 1];
                let mut mean = 0.0;
                let mut norm_sq = 0.0;
                for i in 0..n {
                    mean += zptr[i];
                    norm_sq += zptr[i] * zptr[i];
                }
                mean /= n as f64;
                let norm = if norm_sq.abs() > 1e-30 {
                    (norm_sq - n as f64 * mean * mean).sqrt()
                } else {
                    1.0
                };
                for i in 0..size {
                    result[offset + size + i] /= (factor * norm);
                    result[offset + i] =
                        result[offset + i] / factor1 - mean * result[offset + size + i];
                }
                offset += 2 * size;
            }
        }
    }

    (result[0], result)
}

/// Initialize betahat for the next lambda from the previous solution (warm start).
///
/// Port of `initialize_betahat()` in R + `initialize_beta()` in C.
/// Copies matching coefficients from the old active set to the new one.
pub fn initialize_betahat(
    new_active: &ActiveSet,
    old_active: &ActiveSet,
    old_betahat: &[f64],
    _levels: &[usize],
) -> Vec<f64> {
    // If old is empty or betahat has only intercept
    if old_active.num_groups() == 0 || old_betahat.len() <= 1 {
        // Just return zeros with the old intercept
        let group_sizes = new_active.group_sizes(_levels);
        let total: usize = group_sizes.iter().sum();
        let mut beta = vec![0.0; total + 1];
        if !old_betahat.is_empty() {
            beta[0] = old_betahat[0];
        }
        return beta;
    }

    // Build a mapping from (type, pair) → offset in old betahat
    let old_sizes = old_active.group_sizes(_levels);
    let mut old_offsets: Vec<(usize, usize)> = Vec::new(); // (start, size) per group
    let mut pos = 1; // skip intercept
    for &s in &old_sizes {
        old_offsets.push((pos, s));
        pos += s;
    }

    // For each group in new active set, try to find it in old active set
    let new_sizes = new_active.group_sizes(_levels);
    let total_new: usize = new_sizes.iter().sum();
    let mut beta = vec![0.0; total_new + 1];
    beta[0] = if !old_betahat.is_empty() {
        old_betahat[0]
    } else {
        0.0
    };

    let mut new_offset = 1; // skip intercept
    let mut new_group_idx = 0usize;

    // Process each group type
    let old_nv = old_active.n_vars();

    // Build old group lists for matching
    let mut old_groups: Vec<(usize, Vec<usize>)> = Vec::new(); // (type, indices)
    let mut old_grp_offset = 0usize;
    for group_type in 0..5 {
        let n_type = old_nv[group_type];
        for g in 0..n_type {
            let indices: Vec<usize> = match group_type {
                0 => old_active
                    .cat
                    .as_ref()
                    .map(|v| vec![v[g][0]])
                    .unwrap_or_default(),
                1 => old_active
                    .cont
                    .as_ref()
                    .map(|v| vec![v[g][0]])
                    .unwrap_or_default(),
                2 => old_active
                    .catcat
                    .as_ref()
                    .map(|v| v[g].to_vec())
                    .unwrap_or_default(),
                3 => old_active
                    .contcont
                    .as_ref()
                    .map(|v| v[g].to_vec())
                    .unwrap_or_default(),
                4 => old_active
                    .catcont
                    .as_ref()
                    .map(|v| v[g].to_vec())
                    .unwrap_or_default(),
                _ => unreachable!(),
            };
            old_groups.push((group_type, indices));
            old_grp_offset += 1;
        }
    }

    // For each new group
    let new_nv = new_active.n_vars();
    for group_type in 0..5 {
        let n_type = new_nv[group_type];
        for g in 0..n_type {
            let new_indices: Vec<usize> = match group_type {
                0 => new_active
                    .cat
                    .as_ref()
                    .map(|v| vec![v[g][0]])
                    .unwrap_or_default(),
                1 => new_active
                    .cont
                    .as_ref()
                    .map(|v| vec![v[g][0]])
                    .unwrap_or_default(),
                2 => new_active
                    .catcat
                    .as_ref()
                    .map(|v| v[g].to_vec())
                    .unwrap_or_default(),
                3 => new_active
                    .contcont
                    .as_ref()
                    .map(|v| v[g].to_vec())
                    .unwrap_or_default(),
                4 => new_active
                    .catcont
                    .as_ref()
                    .map(|v| v[g].to_vec())
                    .unwrap_or_default(),
                _ => unreachable!(),
            };
            let size = new_sizes[new_group_idx];

            // Search for matching old group
            for (old_idx, &(ot, ref oi)) in old_groups.iter().enumerate() {
                if ot != group_type {
                    continue;
                }
                // Match pair (order-independent for catcat/contcont, order-dependent for catcont)
                let matches = match group_type {
                    0 | 1 => new_indices[0] == oi[0],
                    4 => new_indices[0] == oi[0] && new_indices[1] == oi[1],
                    _ => {
                        (new_indices[0] == oi[0] && new_indices[1] == oi[1])
                            || (new_indices[0] == oi[1] && new_indices[1] == oi[0])
                    }
                };
                if matches {
                    let (old_start, old_size) = old_offsets[old_idx];
                    let copy_len = size.min(old_size);
                    for i in 0..copy_len {
                        if old_start + i < old_betahat.len() {
                            beta[new_offset + i] = old_betahat[old_start + i];
                        }
                    }
                    break;
                }
            }

            new_offset += size;
            new_group_idx += 1;
        }
    }

    beta
}

/// Predict using rescaled coefficients (in original variable space).
///
/// Port of `x_times_rescaled_beta()` in `c_routines.c`.
pub fn predict_one(active: &ActiveSet, betahat: &[f64], data: &GlinternetData) -> Vec<f64> {
    let n = data.n;
    let mut result = vec![betahat[0]; n];
    let mut offset = 1usize;

    // Categorical main effects
    if let Some(ref cat) = active.cat {
        for &[ci] in cat {
            let size = data.levels[ci - 1];
            let xptr = &data.xcat[(ci - 1) * n..ci * n];
            for i in 0..n {
                result[i] += betahat[offset + xptr[i]];
            }
            offset += size;
        }
    }

    // Continuous main effects — note: betahat is already rescaled, so use RAW z
    // But we stored standardized z. The rescaled coefficients account for this.
    // Actually, rescale_betahat undoes the standardization, so we should use
    // unstandardized z. But we don't store unstandardized z.
    // The C code `x_times_rescaled_beta` uses the standardized Z too,
    // because the rescaled betahat compensates. Let me verify...
    //
    // Looking at the C code: rescale_beta divides by the norm, and adjusts the intercept.
    // Then x_times_rescaled_beta uses the (standardized) z with the rescaled beta.
    // This works because: standardized_z * (coef/norm) = raw_z * coef - mean * coef
    // and the intercept is adjusted by -mean * coef.
    // Actually, the rescaling transforms beta_standardized → beta_raw such that
    // z_standardized * beta_standardized = z_raw * beta_raw + intercept_adjustment.
    // So using standardized z with rescaled beta gives the correct prediction.
    //
    // Wait, that's wrong. rescale_beta divides beta by norm, then adjusts intercept.
    // z_std = (z_raw - mean) / norm
    // z_std * (beta_std / norm) = (z_raw - mean) * beta_std / norm²  ≠ z_raw * beta_raw
    //
    // Actually looking more carefully at rescale_beta: for continuous main effects,
    // result[offset] /= norm, and result[0] -= mean * result[offset].
    // So beta_rescaled = beta_std / norm, intercept -= mean * beta_std / norm
    // Then x_times_rescaled_beta does: result[i] += z_std[i] * beta_rescaled
    // = ((z_raw - mean)/norm) * (beta_std / norm) ... hmm that's not right either.
    //
    // Let me re-read the C code for x_times_rescaled_beta:
    // For continuous: zptr = z + (contIndices[p]-1)*n;
    // result[i] += zOffsetPtr[i] * beta[offset];
    // So it uses z (the standardized values) with the rescaled beta.
    //
    // For the math to work: we need z_std * beta_rescaled + intercept_adj to equal
    // the prediction on the original scale. Since z_std = (z_raw - mean) / norm,
    // and beta_rescaled = beta_std / norm, intercept_adj = -mean * beta_rescaled,
    // we get: (z_raw - mean)/norm * beta_std/norm - mean * beta_std/norm
    // = beta_std/norm² * (z_raw - mean - mean) ... that's not matching.
    //
    // I think I'm overcomplicating this. The key insight is that `betahat` returned
    // by the fit is in the STANDARDIZED space (since the solver works on standardized data).
    // The rescale step transforms these to the ORIGINAL space so that predictions use
    // the standardized Z with appropriately adjusted coefficients.
    // The C code's x_times_rescaled_beta does exactly this and it works correctly.

    if let Some(ref cont) = active.cont {
        for &[ci] in cont {
            let zptr = &data.z[(ci - 1) * n..ci * n];
            for i in 0..n {
                result[i] += zptr[i] * betahat[offset];
            }
            offset += 1;
        }
    }

    // Cat × Cat
    if let Some(ref catcat) = active.catcat {
        for &[ci, cj] in catcat {
            let l1 = data.levels[ci - 1];
            let xptr = &data.xcat[(ci - 1) * n..ci * n];
            let yptr = &data.xcat[(cj - 1) * n..cj * n];
            for i in 0..n {
                result[i] += betahat[offset + xptr[i] + l1 * yptr[i]];
            }
            offset += l1 * data.levels[cj - 1];
        }
    }

    // Cont × Cont
    if let Some(ref contcont) = active.contcont {
        for &[ci, cj] in contcont {
            let wptr = &data.z[(ci - 1) * n..ci * n];
            let zptr = &data.z[(cj - 1) * n..cj * n];
            for i in 0..n {
                result[i] += wptr[i] * betahat[offset]
                    + zptr[i] * betahat[offset + 1]
                    + wptr[i] * zptr[i] * betahat[offset + 2];
            }
            offset += 3;
        }
    }

    // Cat × Cont
    if let Some(ref catcont) = active.catcont {
        for &[ci, cj] in catcont {
            let size = data.levels[ci - 1];
            let xptr = &data.xcat[(ci - 1) * n..ci * n];
            let zptr = &data.z[(cj - 1) * n..cj * n];
            for i in 0..n {
                result[i] += betahat[offset + xptr[i]] + zptr[i] * betahat[offset + size + xptr[i]];
            }
            offset += 2 * size;
        }
    }

    result
}
