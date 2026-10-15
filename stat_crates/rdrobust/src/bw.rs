//! Bandwidth pilot calculations (faithful port of `rdrobust_bw`).

use faer::Mat;

use crate::helpers::*;
use crate::vce::{rdrobust_res, rdrobust_vce};

/// Output of a bandwidth pilot calculation.
pub struct BwPilot {
    pub v: f64,
    pub b: f64,
    pub r: f64,
    pub rate: f64,
}

/// Bandwidth pilot calculation.
///
/// Faithful port of `rdrobust_bw(Y, X, T, Z, C, W, c, o, nu, o_B, h_V, h_B, scale, vce, nnmatch, kernel, dups, dupsid, covs_drop_coll, ginv.tol)`.
///
/// Only the sharp RD path (no covs, no fuzzy) is supported; cluster paths
/// are forwarded to `rdrobust_vce`.
#[allow(clippy::too_many_arguments)]
pub fn rdrobust_bw(
    y: &[f64],
    x: &[f64],
    c: f64,
    o: usize,
    nu: usize,
    o_b: usize,
    h_v: f64,
    h_b: f64,
    scale: f64,
    vce: &str,
    nnmatch: usize,
    kernel: Kernel,
    dups: &[usize],
    dupsid: &[usize],
    cluster: Option<&[f64]>,
) -> BwPilot {
    // ----- V-fit at bandwidth h_V -----
    let w = kernel_weight(x, c, h_v, kernel);
    let ind_v: Vec<usize> = (0..x.len()).filter(|&i| w[i] > 0.0).collect();

    let e_y: Vec<f64> = ind_v.iter().map(|&i| y[i]).collect();
    let e_x: Vec<f64> = ind_v.iter().map(|&i| x[i]).collect();
    let e_w: Vec<f64> = ind_v.iter().map(|&i| w[i]).collect();
    let n_v = ind_v.len();

    // R_V = vandermonde(eX - c, o)
    let e_xmc: Vec<f64> = e_x.iter().map(|&v| v - c).collect();
    let r_v = vandermonde(&e_xmc, o);

    // invG_V = qrXXinv(R_V * sqrt(eW))
    let sqrt_ew: Vec<f64> = e_w.iter().map(|w| w.sqrt()).collect();
    let r_v_sqrt = scale_rows(&r_v, &sqrt_ew);
    let inv_g_v = qr_xx_inv(&r_v_sqrt);

    // D_V = eY (no covs/fuzzy)
    let d_v = Mat::from_fn(n_v, 1, |i, _| e_y[i]);

    // beta_V = invG_V * crossprod(R_V * eW, D_V)
    let rt_w_d = weighted_crossprod(&r_v, &e_w, &d_v);
    let beta_v = &inv_g_v * &rt_w_d;

    // s = 1 (sharp RD, no covs)
    let s = vec![1.0];

    // Residuals
    let predicts_v = &r_v * &beta_v; // n_v × 1
    let hii = vec![0.0; n_v];

    // For NN vce: filter dups/dupsid
    let dups_v: Vec<usize> = if vce == "nn" {
        ind_v.iter().map(|&i| dups[i]).collect()
    } else {
        vec![0; n_v]
    };
    let dupsid_v: Vec<usize> = if vce == "nn" {
        ind_v.iter().map(|&i| dupsid[i]).collect()
    } else {
        vec![0; n_v]
    };

    // Cluster for effective sample
    let e_cluster: Option<Vec<f64>> = cluster.map(|cl| ind_v.iter().map(|&i| cl[i]).collect());
    let e_cluster_ref = e_cluster.as_deref();

    // Predictions for residual computation (extract column 0)
    let predicts_v_col0: Vec<f64> = (0..n_v).map(|i| predicts_v[(i, 0)]).collect();
    let m_mat = Mat::from_fn(n_v, 1, |i, _| predicts_v_col0[i]);

    let crv3 = vce == "crv3" && e_cluster_ref.is_some();
    let crv2 = vce == "crv2" && e_cluster_ref.is_some();

    let res_v = rdrobust_res(
        &e_x,
        &e_y,
        None,
        None,
        &m_mat,
        &hii,
        vce,
        nnmatch,
        &dups_v,
        &dupsid_v,
        o + 1,
        crv3,
        crv2,
        e_cluster_ref.is_some(),
    );

    // VCE: aux = rdrobust_vce(0, s, R_V*eW, res_V, eC, ...)
    let r_v_ew = scale_rows(&r_v, &e_w); // R_V * eW
    let cidx_v: Option<Vec<Vec<usize>>> = e_cluster_ref.map(cluster_idx);

    // For CRV2/CRV3: need sqrtRX and invG
    let sqrt_rx_v: Option<Mat<f64>> = if crv3 || crv2 {
        Some(scale_rows(&r_v, &sqrt_ew)) // R_V * sqrt(eW)
    } else {
        None
    };

    let aux = rdrobust_vce(
        0,
        &s,
        &r_v_ew,
        &res_v,
        e_cluster_ref,
        cidx_v.as_deref(),
        if crv3 || crv2 { Some(&inv_g_v) } else { None },
        sqrt_rx_v.as_ref(),
        crv2,
        None,
    );

    // V_V = (invG_V * aux * invG_V)[nu, nu]  (0-based nu)
    let v_full = &inv_g_v * &aux * &inv_g_v;
    let v_v = v_full[(nu, nu)];

    // v = crossprod(R_V*eW, ((eX-c)/h_V)^(o+1))  [(o+1) × 1]
    let u_powers: Vec<f64> = e_xmc
        .iter()
        .map(|&u| (u / h_v).powi((o + 1) as i32))
        .collect();
    let mut v_vec = vec![0.0; o + 1];
    for k in 0..n_v {
        for j in 0..=o {
            v_vec[j] += r_v[(k, j)] * e_w[k] * u_powers[k];
        }
    }

    // Hp[j] = h_V^(j) for j=0..o  (0-based, R uses j-1 so Hp[1]=h^0=1)
    // BConst = sum_j Hp[j] * (invG_V %*% v)[j], then extract [nu]
    let inv_g_v_v = &inv_g_v * Mat::from_fn(o + 1, 1, |i, _| v_vec[i]);
    // R: (Hp*(invG_V%*%v))[nu+1] → 0-based index nu
    // Wait, the R code computes (Hp*(invG_V%*%v)) as an element-wise product
    // and then extracts element nu+1 (1-based).
    // Let me re-read:
    // Hp = 0; for (j in 1:(o+1)) Hp[j] = h_V^((j-1))
    // BConst = (Hp*(invG_V%*%v))[nu+1]
    // So Hp is a vector of length o+1, with Hp[j] = h_V^(j-1) (1-based j).
    // In 0-based: Hp[i] = h_V^i for i=0..o.
    // invG_V%*%v is (o+1)×1.
    // (Hp*(invG_V%*%v)) is element-wise: Hp[i] * (invG_V%*%v)[i]
    // [nu+1] (1-based) = [nu] (0-based)
    let b_const = h_v.powi(nu as i32) * inv_g_v_v[(nu, 0)];

    // ----- B-fit at bandwidth h_B -----
    let w_b = kernel_weight(x, c, h_b, kernel);
    let ind_b: Vec<usize> = (0..x.len()).filter(|&i| w_b[i] > 0.0).collect();

    let eb_y: Vec<f64> = ind_b.iter().map(|&i| y[i]).collect();
    let eb_x: Vec<f64> = ind_b.iter().map(|&i| x[i]).collect();
    let eb_w: Vec<f64> = ind_b.iter().map(|&i| w_b[i]).collect();
    let n_b = ind_b.len();

    let eb_xmc: Vec<f64> = eb_x.iter().map(|&v| v - c).collect();
    let r_b = vandermonde(&eb_xmc, o_b);

    let sqrt_ebw: Vec<f64> = eb_w.iter().map(|w| w.sqrt()).collect();
    let r_b_sqrt = scale_rows(&r_b, &sqrt_ebw);
    let inv_g_b = qr_xx_inv(&r_b_sqrt);

    let d_b = Mat::from_fn(n_b, 1, |i, _| eb_y[i]);
    let rt_w_d_b = weighted_crossprod(&r_b, &eb_w, &d_b);
    let beta_b = &inv_g_b * &rt_w_d_b;

    let mut bw_reg = 0.0;
    if scale > 0.0 {
        // Residuals at B fit
        let predicts_b = &r_b * &beta_b;
        let predicts_b_col0: Vec<f64> = (0..n_b).map(|i| predicts_b[(i, 0)]).collect();
        let m_b = Mat::from_fn(n_b, 1, |i, _| predicts_b_col0[i]);
        let hii_b = vec![0.0; n_b];

        let dups_b: Vec<usize> = if vce == "nn" {
            ind_b.iter().map(|&i| dups[i]).collect()
        } else {
            vec![0; n_b]
        };
        let dupsid_b: Vec<usize> = if vce == "nn" {
            ind_b.iter().map(|&i| dupsid[i]).collect()
        } else {
            vec![0; n_b]
        };

        let eb_cluster: Option<Vec<f64>> = cluster.map(|cl| ind_b.iter().map(|&i| cl[i]).collect());
        let eb_cluster_ref = eb_cluster.as_deref();

        let res_b = rdrobust_res(
            &eb_x,
            &eb_y,
            None,
            None,
            &m_b,
            &hii_b,
            vce,
            nnmatch,
            &dups_b,
            &dupsid_b,
            o_b + 1,
            crv3,
            crv2,
            eb_cluster_ref.is_some(),
        );

        let r_b_ew = scale_rows(&r_b, &eb_w);
        let cidx_b: Option<Vec<Vec<usize>>> = eb_cluster_ref.map(cluster_idx);

        let sqrt_rx_b: Option<Mat<f64>> = if crv3 || crv2 {
            Some(scale_rows(&r_b, &sqrt_ebw))
        } else {
            None
        };

        let aux_b = rdrobust_vce(
            0,
            &s,
            &r_b_ew,
            &res_b,
            eb_cluster_ref,
            cidx_b.as_deref(),
            if crv3 || crv2 { Some(&inv_g_b) } else { None },
            sqrt_rx_b.as_ref(),
            crv2,
            None,
        );

        let v_b_full = &inv_g_b * &aux_b * &inv_g_b;
        // R: [o+2, o+2] (1-based) → 0-based [o+1, o+1]
        let v_b = v_b_full[(o + 1, o + 1)];
        bw_reg = 3.0 * b_const * b_const * v_b;
    }

    // B = sqrt(2*(o+1-nu)) * BConst * (s' * beta_B[o+2,])
    // For s=1: s' * beta_B[o+2,1] = beta_B[o+1, 0] (0-based)
    let b_val = (2.0 * (o + 1 - nu) as f64).sqrt() * b_const * beta_b[(o + 1, 0)];

    // V = (2*nu+1) * h_V^(2*nu+1) * V_V
    let v_val = (2.0 * nu as f64 + 1.0) * h_v.powi((2 * nu + 1) as i32) * v_v;

    // R = scale * (2*(o+1-nu)) * BWreg
    let r_val = scale * (2.0 * (o + 1 - nu) as f64) * bw_reg;

    // rate = 1/(2*o+3)
    let rate = 1.0 / (2.0 * o as f64 + 3.0);

    BwPilot {
        v: v_val,
        b: b_val,
        r: r_val,
        rate,
    }
}
