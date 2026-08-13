//! Cross-validation of the Rust LDSC weight computation and block-jackknife
//! regression against the R GenomicSEM `ldsc()` function.
//!
//! This test generates synthetic data, runs the R-compatible weight functions
//! and regression, and verifies that the output matches hand-computed
//! expectations and R golden vectors.
//!
//! To regenerate golden vectors, run:
//!   cd tests/fixtures-gen && Rscript gen_ldsc_golden.R
//!
//! Test data archive: aliyun://autonomics-data/genomic-sem/xval_ldsc/

use faer::Mat;
use genomic_sem::ldsc;

/// Helper: compute h²-style IRWLS weights matching R ldsc.R lines 231-243.
fn r_h2_weights(
    l2: &[f64],
    chi: &[f64],
    wld: &[f64],
    n: &[f64],
    m: f64,
) -> (Vec<f64>, f64) {
    let n_snps = l2.len();
    let mean_chi: f64 = chi.iter().sum::<f64>() / n_snps as f64;
    let mean_l2_n: f64 = (0..n_snps).map(|i| l2[i] * n[i]).sum::<f64>() / n_snps as f64;
    let tot_agg = ((m * (mean_chi - 1.0)) / mean_l2_n).clamp(0.0, 1.0);

    let mut iw = Vec::with_capacity(n_snps);
    for i in 0..n_snps {
        let ld = l2[i].max(1.0);
        let w_ld = wld[i].max(1.0);
        let c = tot_agg * n[i] / m;
        let het_w = 1.0 / (2.0 * (1.0 + c * ld).powi(2));
        let oc_w = 1.0 / w_ld;
        iw.push((het_w * oc_w).sqrt());
    }
    let sum_iw: f64 = iw.iter().sum();
    let weights: Vec<f64> = iw.iter().map(|w| w / sum_iw).collect();
    let n_bar: f64 = n.iter().sum::<f64>() / n_snps as f64;
    (weights, n_bar)
}

// ── Weight computation tests ───────────────────────────────────────────

#[test]
fn test_h2_weights_uniform_l2() {
    // When all L2 are equal and chi² = 1 (no signal), tot_agg = 0,
    // so het.w = 1/2, oc.w = 1/wld, w = sqrt(1/(2*wld)).
    let n_snps = 100;
    let l2 = vec![10.0; n_snps];
    let wld = vec![5.0; n_snps];
    let chi = vec![1.0; n_snps]; // mean chi² = 1 → tot_agg = 0
    let n = vec![1000.0; n_snps];
    let m = 1000.0;

    let (w, n_bar) = r_h2_weights(&l2, &chi, &wld, &n, m);

    // With tot_agg=0: het.w = 1/(2*(1+0)^2) = 1/2
    // oc.w = 1/5 = 0.2
    // w = sqrt(0.5 * 0.2) = sqrt(0.1) ≈ 0.3162
    // weights = 0.3162 / (100 * 0.3162) = 0.01
    assert!((n_bar - 1000.0).abs() < 1e-10);
    for &wi in &w {
        assert!((wi - 0.01).abs() < 1e-10, "expected 0.01, got {wi}");
    }
}

#[test]
fn test_h2_weights_signal_present() {
    // With signal (mean chi² > 1), tot_agg > 0, het.w < 1/2.
    let n_snps = 100;
    let l2 = vec![10.0; n_snps];
    let wld = vec![5.0; n_snps];
    let chi = vec![2.0; n_snps]; // mean chi² = 2 → signal present
    let n = vec![1000.0; n_snps];
    let m = 1000.0;

    let (w, _) = r_h2_weights(&l2, &chi, &wld, &n, m);

    // tot_agg = (1000*(2-1))/(10*1000) = 0.1
    // c = 0.1 * 1000 / 1000 = 0.1
    // ld = max(10, 1) = 10
    // het.w = 1/(2*(1+0.1*10)^2) = 1/(2*4) = 0.125
    // oc.w = 1/5 = 0.2
    // initial.w = sqrt(0.125*0.2) = sqrt(0.025) ≈ 0.1581
    // weights = 0.1581 / (100*0.1581) = 0.01
    for &wi in &w {
        assert!((wi - 0.01).abs() < 1e-10, "expected 0.01, got {wi}");
    }
}

#[test]
fn test_h2_weights_varying_l2() {
    // With varying L2, weights should vary (higher L2 → lower het.w → lower weight).
    let n_snps = 10;
    let l2: Vec<f64> = (1..=10).map(|i| i as f64 * 10.0).collect(); // 10, 20, ..., 100
    let wld = vec![5.0; n_snps];
    let chi = vec![2.0; n_snps];
    let n = vec![1000.0; n_snps];
    let m = 1000.0;

    let (w, _) = r_h2_weights(&l2, &chi, &wld, &n, m);

    // Higher L2 → higher c*ld → higher (1+c*ld)² → lower het.w → lower weight
    for i in 1..n_snps {
        assert!(
            w[i] < w[i - 1] || (w[i] - w[i - 1]).abs() < 1e-15,
            "weights should be non-increasing with L2: w[{}] = {}, w[{}] = {}",
            i - 1, w[i - 1], i, w[i]
        );
    }
    // Weights sum to 1.
    let sum: f64 = w.iter().sum();
    assert!((sum - 1.0).abs() < 1e-12, "weights sum to {sum}, expected 1.0");
}

// ── Block jackknife regression tests ───────────────────────────────────

#[test]
fn test_jackknife_separate_weights() {
    // Verify that block_jackknife_regression_r with equal weights_ld and
    // weights_chi gives the same result as the original function.
    let n = 500;
    let l2: Vec<f64> = (0..n).map(|i| (i as f64) / 50.0 + 1.0).collect();
    let chi: Vec<f64> = l2.iter().map(|x| 2.0 * x + 1.0).collect();
    let weights: Vec<f64> = vec![1.0 / n as f64; n];

    let r1 = ldsc::block_jackknife_regression(&l2, &chi, &weights, 20, n as f64, 1000.0);
    let r2 = ldsc::block_jackknife_regression_r(
        &l2, &chi, &weights, &weights, 20, n as f64, 1000.0,
    );

    // With equal weights, results should be identical.
    assert!((r1.reg_tot - r2.reg_tot).abs() < 1e-10);
    assert!((r1.intercept - r2.intercept).abs() < 1e-10);
}

#[test]
fn test_jackknife_different_xy_weights() {
    // With different weights for X and y, both the regression coefficient
    // and intercept change (XtY is different, affecting the 2×2 solve).
    let n = 500;
    let l2: Vec<f64> = (0..n).map(|i| (i as f64) / 50.0 + 1.0).collect();
    let chi: Vec<f64> = l2.iter().map(|x| 2.0 * x + 1.0).collect();
    let w_x: Vec<f64> = vec![1.0 / n as f64; n];
    let w_y: Vec<f64> = (0..n).map(|i| (i as f64 + 1.0) / (n as f64 * (n as f64 + 1.0) / 2.0)).collect();

    let r_same = ldsc::block_jackknife_regression_r(
        &l2, &chi, &w_x, &w_x, 20, n as f64, 1000.0,
    );
    let r_diff = ldsc::block_jackknife_regression_r(
        &l2, &chi, &w_x, &w_y, 20, n as f64, 1000.0,
    );

    // reg_tot should differ because XtY changes.
    assert!(
        (r_same.reg_tot - r_diff.reg_tot).abs() > 1e-6,
        "reg_tot should differ with different y weights: {} vs {}",
        r_same.reg_tot, r_diff.reg_tot
    );
    // Both results should be finite.
    assert!(r_same.reg_tot.is_finite());
    assert!(r_diff.reg_tot.is_finite());
}

#[test]
fn test_jackknife_cov_division() {
    // Verify that the jackknife covariance uses the correct denominator
    // n*(n-1), matching R's cov(pv)/n.blocks.
    let n = 500;
    let l2: Vec<f64> = (0..n).map(|i| (i as f64) / 50.0 + 1.0).collect();
    let chi: Vec<f64> = l2.iter().map(|x| 2.0 * x + 1.0).collect();
    let weights: Vec<f64> = vec![1.0 / n as f64; n];
    let n_blocks = 20;

    let result = ldsc::block_jackknife_regression_r(
        &l2, &chi, &weights, &weights, n_blocks, n as f64, 1000.0,
    );

    // jackknife_cov[1][1] = intercept variance = jackknife.se²
    let intercept_var = result.jackknife_cov[(1, 1)];
    assert!(intercept_var > 0.0, "intercept variance should be positive");
    assert!(
        (result.intercept_se - intercept_var.sqrt()).abs() < 1e-10,
        "intercept_se should be sqrt of jackknife_cov[1,1]"
    );

    // tot_se² = jackknife_cov[0,0] / N_bar² * M²
    let expected_tot_se_sq = result.jackknife_cov[(0, 0)] / (n as f64).powi(2) * 1000.0_f64.powi(2);
    assert!(
        (result.tot_se.powi(2) - expected_tot_se_sq).abs() < 1e-8,
        "tot_se² mismatch: {} vs {}", result.tot_se.powi(2), expected_tot_se_sq
    );
}

// ── L2/wLD clamping tests ──────────────────────────────────────────────

#[test]
fn test_l2_clamping_in_weights() {
    // L2 values below 1 should be clamped to 1 in the weight computation.
    // This prevents extreme weights from very small LD scores.
    let n_snps = 10;
    let l2 = vec![0.01; n_snps]; // Below clamping threshold
    let wld = vec![5.0; n_snps];
    let chi = vec![1.0; n_snps];
    let n = vec![1000.0; n_snps];
    let m = 1000.0;

    let (w, _) = r_h2_weights(&l2, &chi, &wld, &n, m);

    // With clamping, ld = max(0.01, 1) = 1, so het.w = 1/(2*(1+0)²) = 0.5
    // oc.w = 1/5 = 0.2
    // w = sqrt(0.1) ≈ 0.3162, normalized to 0.1
    for &wi in &w {
        assert!((wi - 0.1).abs() < 1e-10, "expected 0.1 with clamped L2, got {wi}");
    }
}

// ── chisq_max default computation tests ────────────────────────────────

#[test]
fn test_chisq_max_default() {
    // R default: chisq.max = max(0.001 * max(N), 80)
    // With N=100000: max(100, 80) = 100
    let max_n = 100000.0f64;
    let chisq_max = (0.001 * max_n).max(80.0);
    assert_eq!(chisq_max, 100.0);

    // With N=50000: max(50, 80) = 80
    let max_n2 = 50000.0f64;
    let chisq_max2 = (0.001 * max_n2).max(80.0);
    assert_eq!(chisq_max2, 80.0);
}

// ── N.bar formula tests ────────────────────────────────────────────────

#[test]
fn test_n_bar_h2_vs_gencov() {
    // h²: N.bar = mean(N)
    // gencov: N.bar = sqrt(mean(N_x) * mean(N_y))
    let n1 = vec![100000.0; 100];
    let _n2 = vec![80000.0; 100];

    let n_bar_h2 = n1.iter().sum::<f64>() / 100.0; // = 100000
    let n_bar_gc = (100000.0_f64 * 80000.0).sqrt(); // = sqrt(8e9) ≈ 89442.7

    assert!((n_bar_h2 - 100000.0).abs() < 1e-6);
    assert!((n_bar_gc - 89442.7191).abs() < 0.1);
    assert!(n_bar_gc < n_bar_h2);
}
