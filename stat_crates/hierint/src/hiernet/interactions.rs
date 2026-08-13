//! Interaction matrix computation and prediction — port of `hierNet.c` helpers.
//!
//! The interaction matrix `zz` is stored as a flat column-major array of size n × cp2,
//! where cp2 = C(p,2) (without diagonal) or C(p,2) + p (with diagonal).

/// Upper-triangular index (without diagonal): j < k.
#[inline]
pub fn ut(j: usize, k: usize, p: usize) -> usize {
    p * j - j * (j + 1) / 2 + k - j - 1
}

/// Upper-triangular index (with diagonal): j ≤ k.
#[inline]
pub fn utd(j: usize, k: usize, p: usize) -> usize {
    ut(j, k + 1, p + 1)
}

/// Compute the n × cp2 interaction matrix ZZ.
///
/// Port of `ComputeInteractionsWithIndices()` / `ComputeInteractionsWithDiagWithIndices()`.
/// Returns a column-major n×cp2 array.
pub fn compute_interactions(x: &[f64], n: usize, p: usize, diagonal: bool) -> Vec<f64> {
    let cp2 = if diagonal {
        p * (p - 1) / 2 + p
    } else {
        p * (p - 1) / 2
    };
    let mut zz = vec![0.0; n * cp2];

    if diagonal {
        // Include diagonal: j ≤ k
        for j in 0..p {
            for k in j..p {
                let jj = utd(j, k, p);
                for i in 0..n {
                    zz[i + n * jj] = x[i + n * j] * x[i + n * k];
                }
            }
        }
    } else {
        // Exclude diagonal: j < k
        for j in 0..p - 1 {
            for k in j + 1..p {
                let jj = ut(j, k, p);
                for i in 0..n {
                    zz[i + n * jj] = x[i + n * j] * x[i + n * k];
                }
            }
        }
    }

    zz
}

/// Compute predicted values yhat from coefficients.
///
/// Port of `compute_yhat_zz()` in `hierNet.c`.
/// yhat[i] = sum_j (bp[j]-bn[j]) * x[i,j] + sum_{j<k} (th[j,k]+th[k,j])/2 * zz[i,ut(j,k)]
///           + sum_j th[j,j] * zz[i,utd(j,j)]  (if diagonal)
pub fn compute_yhat(
    x: &[f64],
    n: usize,
    p: usize,
    zz: &[f64],
    diagonal: bool,
    th: &[f64],
    bp: &[f64],
    bn: &[f64],
) -> Vec<f64> {
    let mut yhat = vec![0.0; n];

    // Main effects
    for j in 0..p {
        let b = bp[j] - bn[j];
        if b != 0.0 {
            for i in 0..n {
                yhat[i] += x[i + n * j] * b;
            }
        }
    }

    // Interaction effects
    if diagonal {
        for j in 0..p - 1 {
            for k in j + 1..p {
                let b = th[j + p * k] + th[k + p * j];
                if b != 0.0 {
                    let jj = utd(j, k, p);
                    for i in 0..n {
                        yhat[i] += zz[i + n * jj] * b / 2.0;
                    }
                }
            }
        }
        // Diagonal terms
        for j in 0..p {
            let b = th[j + p * j];
            if b != 0.0 {
                let jj = utd(j, j, p);
                for i in 0..n {
                    yhat[i] += zz[i + n * jj] * b;
                }
            }
        }
    } else {
        for j in 0..p - 1 {
            for k in j + 1..p {
                let b = th[j + p * k] + th[k + p * j];
                if b != 0.0 {
                    let jj = ut(j, k, p);
                    for i in 0..n {
                        yhat[i] += zz[i + n * jj] * b / 2.0;
                    }
                }
            }
        }
    }

    yhat
}

/// Compute predicted probabilities for logistic.
pub fn compute_phat(
    x: &[f64],
    n: usize,
    p: usize,
    zz: &[f64],
    diagonal: bool,
    b0: f64,
    th: &[f64],
    bp: &[f64],
    bn: &[f64],
) -> Vec<f64> {
    let yhat = compute_yhat(x, n, p, zz, diagonal, th, bp, bn);
    yhat.iter()
        .map(|&yh| 1.0 / (1.0 + (-(b0 + yh)).exp()))
        .collect()
}

/// Compute cross product X^T * v (column-major x: n×p, v: length n).
/// Returns length-p vector.
pub fn cross_prod(x: &[f64], n: usize, p: usize, v: &[f64]) -> Vec<f64> {
    let mut result = vec![0.0; p];
    for j in 0..p {
        let mut dot = 0.0;
        for i in 0..n {
            dot += x[i + n * j] * v[i];
        }
        result[j] = dot;
    }
    result
}

/// Compute dot product <del, grad_th> for a sparse delta.
///
/// Port of `compute_dot_grad_del()` in `hierNet.c`.
pub fn compute_dot_grad_del(
    zz: &[f64],
    diagonal: bool,
    n: usize,
    p: usize,
    r: &[f64],
    del: &[f64],
) -> f64 {
    let mut dotprod = 0.0;

    if diagonal {
        for j in 0..p - 1 {
            for k in j + 1..p {
                let dd = del[j + p * k] + del[k + p * j];
                if dd != 0.0 {
                    let jj = utd(j, k, p);
                    let mut grad = 0.0;
                    for i in 0..n {
                        grad += -zz[i + n * jj] * r[i] / 2.0;
                    }
                    dotprod += dd * grad;
                }
            }
        }
        for j in 0..p {
            let dd = del[j + p * j];
            if dd != 0.0 {
                let jj = utd(j, j, p);
                let mut grad = 0.0;
                for i in 0..n {
                    grad += -zz[i + n * jj] * r[i];
                }
                dotprod += dd * grad;
            }
        }
    } else {
        for j in 0..p - 1 {
            for k in j + 1..p {
                let dd = del[j + p * k] + del[k + p * j];
                if dd != 0.0 {
                    let jj = ut(j, k, p);
                    let mut grad = 0.0;
                    for i in 0..n {
                        grad += -zz[i + n * jj] * r[i] / 2.0;
                    }
                    dotprod += dd * grad;
                }
            }
        }
    }

    dotprod
}
