//! Index/subset helpers — port of `R/indexS.R`, `R/subSV.R`, `R/localSRMD.R`.

use faer::Mat;

/// Create an index matrix for mapping S entries to V entries.
///
/// For a covariance matrix (R=FALSE): lower triangle including diagonal.
/// For a correlation matrix (R=TRUE): lower triangle excluding diagonal.
pub fn index_s(k: usize, is_correlation: bool) -> Mat<f64> {
    let mut s_num = Mat::zeros(k, k);
    if is_correlation {
        let mut counter = 1.0f64;
        for col in 0..k {
            for row in (col + 1)..k {
                s_num[(row, col)] = counter;
                s_num[(col, row)] = counter;
                counter += 1.0;
            }
        }
    } else {
        let mut counter = 1.0f64;
        for col in 0..k {
            for row in col..k {
                s_num[(row, col)] = counter;
                s_num[(col, row)] = counter;
                counter += 1.0;
            }
        }
    }
    s_num
}

/// Subset S and V matrices to specific indices.
pub fn sub_sv(s: &Mat<f64>, v: &Mat<f64>, indices: &[usize]) -> (Mat<f64>, Mat<f64>) {
    let k = indices.len();
    let z = k * (k + 1) / 2;

    // Subset S
    let mut s_sub = Mat::zeros(k, k);
    for (i, &ri) in indices.iter().enumerate() {
        for (j, &rj) in indices.iter().enumerate() {
            s_sub[(i, j)] = s[(ri, rj)];
        }
    }

    // Subset V (vech indices)
    let n_orig = s.nrows();
    let mut vech_indices = Vec::with_capacity(z);
    for col in 0..k {
        for row in col..k {
            let orig_row = indices[row];
            let orig_col = indices[col];
            let orig_vech_idx = orig_col * (2 * n_orig - orig_col - 1) / 2 + orig_row;
            vech_indices.push(orig_vech_idx);
        }
    }

    let mut v_sub = Mat::zeros(z, z);
    for i in 0..z {
        for j in 0..z {
            v_sub[(i, j)] = v[(vech_indices[i], vech_indices[j])];
        }
    }

    (s_sub, v_sub)
}

/// Calculate local SRMD (Standardized Root Mean Deviation).
///
/// `localSRMD = sqrt(mean(((unconstrained - constrained) / (lhs_pooled_sd * rhs_pooled_sd))^2))`
pub fn local_srmd(
    unconstrained: &[f64],
    constrained: &[f64],
    lhs_var: &[Vec<f64>],
    rhs_var: &[Vec<f64>],
) -> f64 {
    let lhs_pooled_sd: Vec<f64> = lhs_var
        .iter()
        .map(|v| v.iter().sum::<f64>().max(0.0) / v.len() as f64)
        .map(|v| v.sqrt())
        .collect();
    let rhs_pooled_sd: Vec<f64> = rhs_var
        .iter()
        .map(|v| v.iter().sum::<f64>().max(0.0) / v.len() as f64)
        .map(|v| v.sqrt())
        .collect();

    let mut sum_sq = 0.0;
    let mut count = 0;
    for i in 0..unconstrained.len() {
        if i < lhs_pooled_sd.len() && i < rhs_pooled_sd.len() {
            let denom = lhs_pooled_sd[i] * rhs_pooled_sd[i];
            if denom > 0.0 {
                let diff = (unconstrained[i] - constrained[i]) / denom;
                sum_sq += diff * diff;
                count += 1;
            }
        }
    }

    if count > 0 {
        (sum_sq / count as f64).sqrt()
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_index_s_covariance() {
        let idx = index_s(3, false);
        // Diagonal should be 1, 4, 6 (column-major lower triangle)
        assert_eq!(idx[(0, 0)], 1.0);
        assert_eq!(idx[(1, 1)], 4.0);
        assert_eq!(idx[(2, 2)], 6.0);
    }

    #[test]
    fn test_index_s_correlation() {
        let idx = index_s(3, true);
        // Diagonal should be 0
        assert_eq!(idx[(0, 0)], 0.0);
        // Off-diagonal should be 1, 2, 3
        assert_eq!(idx[(1, 0)], 1.0);
        assert_eq!(idx[(2, 0)], 2.0);
        assert_eq!(idx[(2, 1)], 3.0);
    }

    #[test]
    fn test_sub_sv() {
        let mut s = Mat::zeros(4, 4);
        for i in 0..4 {
            s[(i, i)] = (i + 1) as f64;
        }
        let v = Mat::<f64>::identity(10, 10);
        let (s_sub, v_sub) = sub_sv(&s, &v, &[0, 2]);
        assert_eq!(s_sub.nrows(), 2);
        assert_eq!(v_sub.nrows(), 3);
    }
}
