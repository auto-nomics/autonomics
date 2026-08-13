//! Residual computation and variance-covariance estimation.
//!
//! Faithful port of `rdrobust_res` and `rdrobust_vce`.

use faer::Mat;

use crate::helpers::*;

// =====================================================================
// Residuals (faithful port of rdrobust_res)
// =====================================================================

#[allow(clippy::too_many_arguments)]
pub fn rdrobust_res(
    x: &[f64],
    y: &[f64],
    t: Option<&[f64]>,
    z: Option<&Mat<f64>>,
    m: &Mat<f64>,
    hii: &[f64],
    vce: &str,
    matches: usize,
    dups: &[usize],
    dupsid: &[usize],
    d: usize,
    crv3: bool,
    crv2: bool,
    has_cluster: bool,
) -> Mat<f64> {
    let n = y.len();
    let mut d_t = 0;
    let mut d_z = 0;
    if t.is_some() {
        d_t = 1;
    }
    if let Some(zm) = z {
        d_z = zm.ncols();
    }
    let ncol = 1 + d_t + d_z;
    let mut res = Mat::zeros(n, ncol);

    if vce == "nn" {
        let nn_tol_eps = f64::EPSILON.sqrt();
        for pos in 1..=n {
            let pi = pos - 1;
            let mut rpos = dups[pi] - dupsid[pi];
            let mut lpos = dupsid[pi] - 1;

            while lpos + rpos < matches.min(n - 1) {
                if pos as i64 - lpos as i64 - 1 <= 0 {
                    rpos += dups[pi + rpos + 1];
                } else if pos + rpos + 1 > n {
                    lpos += dups[pi - lpos - 1];
                } else {
                    let dleft = x[pi] - x[pi - lpos - 1];
                    let dright = x[pi + rpos + 1] - x[pi];
                    let nn_tol = dleft.max(dright) * nn_tol_eps;
                    if dleft - dright > nn_tol {
                        rpos += dups[pi + rpos + 1];
                    } else if dright - dleft > nn_tol {
                        lpos += dups[pi - lpos - 1];
                    } else {
                        rpos += dups[pi + rpos + 1];
                        lpos += dups[pi - lpos - 1];
                    }
                }
            }

            let lo = (pos as i64 - lpos as i64).max(0) as usize;
            let hi = (pos + rpos).min(n);

            // Y residual
            let mut y_j = 0.0;
            let mut ji = 0usize;
            for j in lo..=hi {
                if j >= 1 && j != pos {
                    y_j += y[j - 1];
                    ji += 1;
                }
            }
            let ji_f = ji as f64;
            if ji_f > 0.0 {
                res[(pi, 0)] = (ji_f / (ji_f + 1.0)).sqrt() * (y[pi] - y_j / ji_f);
            }

            // Treatment residual (fuzzy)
            if let Some(tv) = t {
                let mut t_j = 0.0;
                let mut jit = 0usize;
                for j in lo..=hi {
                    if j >= 1 && j != pos {
                        t_j += tv[j - 1];
                        jit += 1;
                    }
                }
                let jf = jit as f64;
                if jf > 0.0 {
                    res[(pi, 1)] = (jf / (jf + 1.0)).sqrt() * (tv[pi] - t_j / jf);
                }
            }

            // Covariate residuals
            if let Some(zm) = z {
                for i in 0..d_z {
                    let mut z_j = 0.0;
                    let mut jiz = 0usize;
                    for j in lo..=hi {
                        if j >= 1 && j != pos {
                            z_j += zm[(j - 1, i)];
                            jiz += 1;
                        }
                    }
                    let jf = jiz as f64;
                    if jf > 0.0 {
                        res[(pi, 1 + d_t + i)] =
                            (jf / (jf + 1.0)).sqrt() * (zm[(pi, i)] - z_j / jf);
                    }
                }
            }
        }
    } else if crv3 || crv2 {
        for i in 0..n {
            res[(i, 0)] = y[i] - m[(i, 0)];
            if d_t == 1 {
                if let Some(tv) = t {
                    res[(i, 1)] = tv[i] - m[(i, 1)];
                }
            }
            if d_z > 0 {
                if let Some(zm) = z {
                    for k in 0..d_z {
                        res[(i, 1 + d_t + k)] = zm[(i, k)] - m[(i, 1 + d_t + k)];
                    }
                }
            }
        }
    } else {
        let w: Vec<f64> = match vce {
            "hc0" => vec![1.0; n],
            "hc1" => {
                if has_cluster {
                    vec![1.0; n]
                } else {
                    let f = (n as f64 / (n - d) as f64).sqrt();
                    vec![f; n]
                }
            }
            "hc2" => hii
                .iter()
                .map(|&h| (1.0 - h).max(1e-8).recip().sqrt())
                .collect(),
            _ => hii.iter().map(|&h| (1.0 - h).max(1e-8).recip()).collect(),
        };
        for i in 0..n {
            res[(i, 0)] = w[i] * (y[i] - m[(i, 0)]);
            if d_t == 1 {
                if let Some(tv) = t {
                    res[(i, 1)] = w[i] * (tv[i] - m[(i, 1)]);
                }
            }
            if d_z > 0 {
                if let Some(zm) = z {
                    for k in 0..d_z {
                        res[(i, 1 + d_t + k)] = w[i] * (zm[(i, k)] - m[(i, 1 + d_t + k)]);
                    }
                }
            }
        }
    }
    res
}

// =====================================================================
// Variance-covariance (faithful port of rdrobust_vce)
// =====================================================================

#[allow(clippy::too_many_arguments)]
pub fn rdrobust_vce(
    d: usize,
    s: &[f64],
    rx: &Mat<f64>,
    res: &Mat<f64>,
    cluster: Option<&[f64]>,
    cluster_idx_opt: Option<&[Vec<usize>]>,
    inv_g: Option<&Mat<f64>>,
    sqrt_rx: Option<&Mat<f64>>,
    crv2: bool,
    k_override: Option<usize>,
) -> Mat<f64> {
    let k = rx.ncols();
    let k_df = k_override.unwrap_or(k);
    let mut meat = Mat::zeros(k, k);

    let crv3_mode = inv_g.is_some() && cluster.is_some() && !crv2;
    let crv2_mode = inv_g.is_some() && cluster.is_some() && crv2;

    if cluster.is_none() {
        // Non-cluster
        let n = rx.nrows();
        if d == 0 {
            for i in 0..k {
                for j in i..k {
                    let mut sum = 0.0;
                    for nn in 0..n {
                        sum += res[(nn, 0)].powi(2) * rx[(nn, i)] * rx[(nn, j)];
                    }
                    meat[(i, j)] = sum;
                    if i != j {
                        meat[(j, i)] = sum;
                    }
                }
            }
        } else {
            let mut r_comb = vec![0.0; n];
            for nn in 0..n {
                for l in 0..(1 + d) {
                    r_comb[nn] += res[(nn, l)] * s[l];
                }
            }
            for i in 0..k {
                for j in i..k {
                    let mut sum = 0.0;
                    for nn in 0..n {
                        sum += r_comb[nn].powi(2) * rx[(nn, i)] * rx[(nn, j)];
                    }
                    meat[(i, j)] = sum;
                    if i != j {
                        meat[(j, i)] = sum;
                    }
                }
            }
        }
        return meat;
    }

    // Cluster paths
    let c = cluster.unwrap();
    let cidx: Vec<Vec<usize>> = match cluster_idx_opt {
        Some(idx) => idx.to_vec(),
        None => cluster_idx(c),
    };
    let g = cidx.len();
    let n = c.len();

    if crv3_mode {
        let inv_g_mat = inv_g.unwrap();
        let g_mat = lu_inverse(inv_g_mat);
        let has_sqrt_rx = sqrt_rx.is_some();

        for indices in &cidx {
            let score = crv3_cluster_score(indices, d, s, rx, res, sqrt_rx, has_sqrt_rx, &g_mat, k);
            add_tcrossprod(&mut meat, &score, k);
        }
        return meat;
    }

    if crv2_mode {
        let inv_g_mat = inv_g.unwrap();
        let c_half_opt = spd_inverse(inv_g_mat); // chol(invG) via LLT

        for indices in &cidx {
            let score = if c_half_opt.is_some() {
                crv2_cluster_score(
                    indices,
                    d,
                    s,
                    rx,
                    res,
                    sqrt_rx,
                    c_half_opt.as_ref().unwrap(),
                    k,
                )
            } else {
                simple_cluster_score(indices, d, s, rx, res)
            };
            add_tcrossprod(&mut meat, &score, k);
        }
        return meat;
    }

    // CR1
    let w = ((n - 1) as f64 / (n - k_df) as f64) * (g as f64 / (g - 1) as f64);
    for indices in &cidx {
        let score = simple_cluster_score(indices, d, s, rx, res);
        add_tcrossprod(&mut meat, &score, k);
    }
    for i in 0..k {
        for j in 0..k {
            meat[(i, j)] *= w;
        }
    }
    meat
}

fn add_tcrossprod(meat: &mut Mat<f64>, score: &[f64], k: usize) {
    for i in 0..k {
        for j in i..k {
            let val = score[i] * score[j];
            meat[(i, j)] += val;
            if i != j {
                meat[(j, i)] += val;
            }
        }
    }
}

fn simple_cluster_score(
    indices: &[usize],
    d: usize,
    s: &[f64],
    rx: &Mat<f64>,
    res: &Mat<f64>,
) -> Vec<f64> {
    let k = rx.ncols();
    let mut sv = vec![0.0; k];
    for &idx in indices {
        if d == 0 {
            for j in 0..k {
                sv[j] += rx[(idx, j)] * res[(idx, 0)];
            }
        } else {
            let mut r_comb = 0.0;
            for l in 0..(1 + d) {
                r_comb += res[(idx, l)] * s[l];
            }
            for j in 0..k {
                sv[j] += rx[(idx, j)] * r_comb;
            }
        }
    }
    sv
}

#[allow(clippy::too_many_arguments)]
fn crv3_cluster_score(
    indices: &[usize],
    d: usize,
    s: &[f64],
    rx: &Mat<f64>,
    res: &Mat<f64>,
    sqrt_rx: Option<&Mat<f64>>,
    has_sqrt_rx: bool,
    g_mat: &Mat<f64>,
    k: usize,
) -> Vec<f64> {
    let srx = sqrt_rx.unwrap_or(rx);

    // L_g
    let mut l_g = Mat::zeros(k, k);
    for a in 0..k {
        for b in a..k {
            let mut sum = 0.0;
            for &idx in indices {
                sum += srx[(idx, a)] * srx[(idx, b)];
            }
            l_g[(a, b)] = sum;
            if a != b {
                l_g[(b, a)] = sum;
            }
        }
    }

    // u_g
    let mut u_g = vec![0.0; k];
    if d == 0 {
        for a in 0..k {
            for &idx in indices {
                if has_sqrt_rx {
                    u_g[a] += srx[(idx, a)] * srx[(idx, 0)] * res[(idx, 0)];
                } else {
                    u_g[a] += rx[(idx, a)] * res[(idx, 0)];
                }
            }
        }
    } else {
        for a in 0..k {
            for &idx in indices {
                let mut r_comb = 0.0;
                for l in 0..(1 + d) {
                    r_comb += res[(idx, l)] * s[l];
                }
                if has_sqrt_rx {
                    u_g[a] += srx[(idx, a)] * srx[(idx, 0)] * r_comb;
                } else {
                    u_g[a] += rx[(idx, a)] * r_comb;
                }
            }
        }
    }

    // G - L_g
    let mut gm_l = Mat::zeros(k, k);
    for a in 0..k {
        for b in 0..k {
            gm_l[(a, b)] = g_mat[(a, b)] - l_g[(a, b)];
        }
    }

    let m_g = lu_inverse(&gm_l);

    // M_g * u_g
    let mut m_u = vec![0.0; k];
    for a in 0..k {
        for b in 0..k {
            m_u[a] += m_g[(a, b)] * u_g[b];
        }
    }

    // score = u_g + L_g * m_u
    let mut score = u_g;
    for a in 0..k {
        for b in 0..k {
            score[a] += l_g[(a, b)] * m_u[b];
        }
    }
    score
}

#[allow(clippy::too_many_arguments)]
fn crv2_cluster_score(
    indices: &[usize],
    d: usize,
    s: &[f64],
    rx: &Mat<f64>,
    res: &Mat<f64>,
    sqrt_rx: Option<&Mat<f64>>,
    c_half: &Mat<f64>,
    k: usize,
) -> Vec<f64> {
    let srx = sqrt_rx.unwrap_or(rx);
    let has_sqrt_rx = sqrt_rx.is_some();
    let t_c_half = c_half.transpose();

    // L_g
    let mut l_g = Mat::zeros(k, k);
    for a in 0..k {
        for b in a..k {
            let mut sum = 0.0;
            for &idx in indices {
                sum += srx[(idx, a)] * srx[(idx, b)];
            }
            l_g[(a, b)] = sum;
            if a != b {
                l_g[(b, a)] = sum;
            }
        }
    }

    // u_g
    let mut u_g = vec![0.0; k];
    if d == 0 {
        for a in 0..k {
            for &idx in indices {
                if has_sqrt_rx {
                    u_g[a] += srx[(idx, a)] * srx[(idx, 0)] * res[(idx, 0)];
                } else {
                    u_g[a] += rx[(idx, a)] * res[(idx, 0)];
                }
            }
        }
    } else {
        for a in 0..k {
            for &idx in indices {
                let mut r_comb = 0.0;
                for l in 0..(1 + d) {
                    r_comb += res[(idx, l)] * s[l];
                }
                if has_sqrt_rx {
                    u_g[a] += srx[(idx, a)] * srx[(idx, 0)] * r_comb;
                } else {
                    u_g[a] += rx[(idx, a)] * r_comb;
                }
            }
        }
    }

    // F_sq = C_half * L_g * t(C_half)
    let f_sq = c_half * &l_g * &t_c_half;

    // Eigen decomposition
    let (eigvals_asc, eigvecs) = sym_eigen(&f_sq).unwrap_or((vec![0.0; k], Mat::zeros(k, k)));
    let sigma2: Vec<f64> = eigvals_asc.iter().map(|&v| v.max(0.0)).collect();

    let c_coef: Vec<f64> = sigma2
        .iter()
        .map(|&s2| {
            if s2 < 1e-14 {
                0.0
            } else {
                ((1.0 - s2).max(1e-8).recip().sqrt() - 1.0) / s2
            }
        })
        .collect();

    // Ru_g = C_half * u_g
    let mut ru_g = vec![0.0; k];
    for a in 0..k {
        for b in 0..k {
            ru_g[a] += c_half[(a, b)] * u_g[b];
        }
    }

    // V' * Ru_g
    let mut vt_ru = vec![0.0; k];
    for a in 0..k {
        for b in 0..k {
            vt_ru[a] += eigvecs[(b, a)] * ru_g[b];
        }
    }

    // adj = V * (c_coef .* vt_ru)
    let mut adj = vec![0.0; k];
    for a in 0..k {
        for b in 0..k {
            adj[a] += eigvecs[(a, b)] * c_coef[b] * vt_ru[b];
        }
    }

    // tC_half * adj
    let mut tc_adj = vec![0.0; k];
    for a in 0..k {
        for b in 0..k {
            tc_adj[a] += t_c_half[(a, b)] * adj[b];
        }
    }

    // L_g * tc_adj
    let mut l_tc_adj = vec![0.0; k];
    for a in 0..k {
        for b in 0..k {
            l_tc_adj[a] += l_g[(a, b)] * tc_adj[b];
        }
    }

    let mut score = u_g;
    for a in 0..k {
        score[a] += l_tc_adj[a];
    }
    score
}
