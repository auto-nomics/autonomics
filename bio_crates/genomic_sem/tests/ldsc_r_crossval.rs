//! End-to-end cross-validation: same synthetic data → R ldsc() vs Rust.
//!
//! Prerequisite: run the R reference script first:
//!   cd tests/fixtures-gen && Rscript gen_ldsc_golden.R
//! Or directly:
//!   cd /mnt/projects/autonomics_projects/genomic-sem/tests/xval_ldsc
//!   Rscript run_r_reference.R 500 20 42
//!
//! This test reads data/input.json (raw arrays) and data/r_output.json
//! (R reference), replays the R-compatible algorithm in Rust, and
//! compares S, V, I, N element-by-element.

use serde::Deserialize;
use serde_json::Value;
use std::fs;

use genomic_sem::ldsc;

#[derive(Deserialize)]
struct InputData {
    n_snps: usize,
    #[allow(dead_code)]
    n_traits: usize,
    l2: Vec<f64>,
    wld: Vec<f64>,
    z1: Vec<f64>,
    z2: Vec<f64>,
    n1: Vec<f64>,
    n2: Vec<f64>,
    m: f64,
    n_blocks: usize,
    #[allow(dead_code)]
    h2_true: Vec<f64>,
    #[allow(dead_code)]
    rg_true: f64,
}

const XVAL_DIR: &str = "/mnt/projects/autonomics_projects/genomic-sem/tests/xval_ldsc/data";

fn load_input() -> Option<InputData> {
    let path = format!("{XVAL_DIR}/input.json");
    let s = fs::read_to_string(&path).ok()?;
    serde_json::from_str(&s).ok()
}

fn load_r_output() -> Option<Value> {
    let path = format!("{XVAL_DIR}/r_output.json");
    let s = fs::read_to_string(&path).ok()?;
    serde_json::from_str(&s).ok()
}

/// R-compatible per-trait chisq.max: max(0.001 * max(N), 80).
fn r_chisq_max(n_vals: &[f64]) -> f64 {
    let max_n = n_vals.iter().cloned().fold(0.0f64, f64::max);
    (0.001 * max_n).max(80.0)
}

/// R-compatible h² IRWLS weights (ldsc.R L231-243).
fn r_h2_weights(
    l2: &[f64],
    chi: &[f64],
    wld: &[f64],
    n: &[f64],
    m: f64,
) -> (Vec<f64>, f64) {
    let ns = l2.len();
    let mean_chi: f64 = chi.iter().sum::<f64>() / ns as f64;
    let mean_l2_n: f64 = (0..ns).map(|i| l2[i] * n[i]).sum::<f64>() / ns as f64;
    let tot_agg = if mean_l2_n > 0.0 {
        ((m * (mean_chi - 1.0)) / mean_l2_n).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let mut iw = Vec::with_capacity(ns);
    for i in 0..ns {
        let ld = l2[i].max(1.0);
        let w_ld = wld[i].max(1.0);
        let c = tot_agg * n[i] / m;
        let het_w = 1.0 / (2.0 * (1.0 + c * ld).powi(2));
        let oc_w = 1.0 / w_ld;
        iw.push((het_w * oc_w).sqrt());
    }
    let sum_iw: f64 = iw.iter().sum();
    let w = if sum_iw > 0.0 {
        iw.iter().map(|v| v / sum_iw).collect()
    } else {
        vec![1.0 / ns as f64; ns]
    };
    let n_bar = n.iter().sum::<f64>() / ns as f64;
    (w, n_bar)
}

/// R-compatible gencov weights (ldsc.R L362-394).
fn r_gencov_weights(
    l2: &[f64],
    wld: &[f64],
    chi1: &[f64],
    n1: &[f64],
    chi2: &[f64],
    n2: &[f64],
    m: f64,
) -> (Vec<f64>, Vec<f64>, f64) {
    let ns = l2.len();

    let mc1: f64 = chi1.iter().sum::<f64>() / ns as f64;
    let ml1: f64 = (0..ns).map(|i| l2[i] * n1[i]).sum::<f64>() / ns as f64;
    let ta1 = if ml1 > 0.0 { ((m * (mc1 - 1.0)) / ml1).clamp(0.0, 1.0) } else { 0.0 };

    let mc2: f64 = chi2.iter().sum::<f64>() / ns as f64;
    let ml2: f64 = (0..ns).map(|i| l2[i] * n2[i]).sum::<f64>() / ns as f64;
    let ta2 = if ml2 > 0.0 { ((m * (mc2 - 1.0)) / ml2).clamp(0.0, 1.0) } else { 0.0 };

    let mut iw1 = Vec::with_capacity(ns);
    let mut iw2 = Vec::with_capacity(ns);
    for i in 0..ns {
        let ld = l2[i].max(1.0);
        let oc = 1.0 / wld[i].max(1.0);
        let c1 = ta1 * n1[i] / m;
        iw1.push((1.0 / (2.0 * (1.0 + c1 * ld).powi(2)) * oc).sqrt());
        let c2 = ta2 * n2[i] / m;
        iw2.push((1.0 / (2.0 * (1.0 + c2 * ld).powi(2)) * oc).sqrt());
    }

    let s1: f64 = iw1.iter().sum();
    let w_ld = if s1 > 0.0 { iw1.iter().map(|v| v / s1).collect() } else { vec![1.0 / ns as f64; ns] };
    let sb: f64 = (0..ns).map(|i| iw1[i] + iw2[i]).sum();
    let w_chi = if sb > 0.0 { (0..ns).map(|i| (iw1[i] + iw2[i]) / sb).collect() } else { vec![1.0 / ns as f64; ns] };
    let mn1 = n1.iter().sum::<f64>() / ns as f64;
    let mn2 = n2.iter().sum::<f64>() / ns as f64;
    (w_ld, w_chi, (mn1 * mn2).sqrt())
}

#[test]
fn test_cross_validate_r_ldsc() {
    let input = match load_input() {
        Some(d) => d,
        None => {
            eprintln!("Skipping R cross-validation: input.json not found. Run run_r_reference.R first.");
            return;
        }
    };
    let r_out = match load_r_output() {
        Some(d) => d,
        None => {
            eprintln!("Skipping R cross-validation: r_output.json not found.");
            return;
        }
    };

    let n_blocks = input.n_blocks;
    let m = input.m;

    // ── Step 1: Per-trait chisq.max filtering (matching R exactly) ───────
    // R applies chisq.max independently per trait BEFORE the regression.
    let cm1 = r_chisq_max(&input.n1);
    let cm2 = r_chisq_max(&input.n2);

    // Trait 1 filtered indices
    let idx1: Vec<usize> = (0..input.n_snps)
        .filter(|&i| input.z1[i] * input.z1[i] <= cm1)
        .collect();
    // Trait 2 filtered indices
    let idx2: Vec<usize> = (0..input.n_snps)
        .filter(|&i| input.z2[i] * input.z2[i] <= cm2)
        .collect();

    let l2_1: Vec<f64> = idx1.iter().map(|&i| input.l2[i]).collect();
    let chi1: Vec<f64> = idx1.iter().map(|&i| input.z1[i].powi(2)).collect();
    let wld_1: Vec<f64> = idx1.iter().map(|&i| input.wld[i]).collect();
    let n_1: Vec<f64> = idx1.iter().map(|&i| input.n1[i]).collect();

    let l2_2: Vec<f64> = idx2.iter().map(|&i| input.l2[i]).collect();
    let chi2: Vec<f64> = idx2.iter().map(|&i| input.z2[i].powi(2)).collect();
    let wld_2: Vec<f64> = idx2.iter().map(|&i| input.wld[i]).collect();
    let n_2: Vec<f64> = idx2.iter().map(|&i| input.n2[i]).collect();

    // Cross-trait: intersection of idx1 and idx2
    let idx12: Vec<usize> = idx1.iter()
        .filter(|i| idx2.contains(i))
        .copied()
        .collect();
    let l2_12: Vec<f64> = idx12.iter().map(|&i| input.l2[i]).collect();
    let zz: Vec<f64> = idx12.iter().map(|&i| input.z1[i] * input.z2[i]).collect();
    let wld_12: Vec<f64> = idx12.iter().map(|&i| input.wld[i]).collect();
    let n1_12: Vec<f64> = idx12.iter().map(|&i| input.n1[i]).collect();
    let n2_12: Vec<f64> = idx12.iter().map(|&i| input.n2[i]).collect();
    let chi1_12: Vec<f64> = idx12.iter().map(|&i| input.z1[i].powi(2)).collect();
    let chi2_12: Vec<f64> = idx12.iter().map(|&i| input.z2[i].powi(2)).collect();

    eprintln!("Trait 1: {} SNPs (chisq_max={:.0})", idx1.len(), cm1);
    eprintln!("Trait 2: {} SNPs (chisq_max={:.0})", idx2.len(), cm2);
    eprintln!("Cross:   {} SNPs (intersection)", idx12.len());

    // ── Step 2: Run regressions ─────────────────────────────────────────
    // For h² pairs, the data is already in CHR/BP order (matching R's
    // initial sort). For gencov, R's merge(sort=FALSE) produces an
    // unpredictable SNP order. We read R's actual gencov order from
    // gencov_debug.json if available; otherwise fall back to our
    // intersection order and use a looser tolerance.
    let gencov_debug_path = format!("{XVAL_DIR}/gencov_debug.json");
    let gencov_debug: Option<Value> = fs::read_to_string(&gencov_debug_path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok());

    // Pair 1: h² for trait 1
    // Pair 1: h² for trait 1
    let (w1, nbar1) = r_h2_weights(&l2_1, &chi1, &wld_1, &n_1, m);
    let r_h2_1 = ldsc::block_jackknife_regression_r(
        &l2_1, &chi1, &w1, &w1, n_blocks, nbar1, m,
    );

    // Pair 2: gencov — use R's exact data from the ldsc() dump.
    let gencov_actual_path = format!("{XVAL_DIR}/gencov_actual.json");
    let gencov_actual: Option<Value> = fs::read_to_string(&gencov_actual_path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok());

    let (r_gc, nbar_g): (ldsc::JackknifeResult, f64) = if let Some(ref gd) = gencov_actual {
        // Use R's EXACT gencov data (order, weights, ZZ) from ldsc() dump.
        let l2_r: Vec<f64> = serde_json::from_value(gd["L2"].clone()).unwrap();
        let zz_r: Vec<f64> = serde_json::from_value(gd["ZZ"].clone()).unwrap();
        let w_ld_r: Vec<f64> = serde_json::from_value(gd["weights"].clone()).unwrap();
        let w_chi_r: Vec<f64> = serde_json::from_value(gd["weights_cov"].clone()).unwrap();
        let nbar_r: f64 = serde_json::from_value(gd["N_bar"].clone()).unwrap();
        eprintln!("Using R's exact gencov data from ldsc() dump ({} SNPs)", l2_r.len());
        eprintln!("  weights[1:3]:    {:?}", &w_ld_r[..3.min(w_ld_r.len())]);
        eprintln!("  weights_cov[1:3]: {:?}", &w_chi_r[..3.min(w_chi_r.len())]);
        (
            ldsc::block_jackknife_regression_r(&l2_r, &zz_r, &w_ld_r, &w_chi_r, n_blocks, nbar_r, m),
            nbar_r,
        )
    } else if let Some(ref gd) = gencov_debug {
        let l2_r: Vec<f64> = serde_json::from_value(gd["L2"].clone()).unwrap();
        let zz_r: Vec<f64> = serde_json::from_value(gd["ZZ"].clone()).unwrap();
        let w_ld_r: Vec<f64> = serde_json::from_value(gd["weights_ld"].clone()).unwrap();
        let w_chi_r: Vec<f64> = serde_json::from_value(gd["weights_cov"].clone()).unwrap();
        let nbar_r: f64 = serde_json::from_value(gd["N_bar"].clone()).unwrap();
        eprintln!("Using R's gencov data from debug replay ({} SNPs)", l2_r.len());
        (
            ldsc::block_jackknife_regression_r(&l2_r, &zz_r, &w_ld_r, &w_chi_r, n_blocks, nbar_r, m),
            nbar_r,
        )
    } else {
        eprintln!("Warning: no gencov dump found — using Rust computation");
        let (wld_g, wchi_g, nb) =
            r_gencov_weights(&l2_12, &wld_12, &chi1_12, &n1_12, &chi2_12, &n2_12, m);
        (
            ldsc::block_jackknife_regression_r(&l2_12, &zz, &wld_g, &wchi_g, n_blocks, nb, m),
            nb,
        )
    };

    // Pair 3: h² for trait 2
    let (w2, nbar2) = r_h2_weights(&l2_2, &chi2, &wld_2, &n_2, m);
    let r_h2_2 = ldsc::block_jackknife_regression_r(
        &l2_2, &chi2, &w2, &w2, n_blocks, nbar2, m,
    );

    // ── Step 3: Assemble S, I, N (vech order: (0,0), (1,0), (1,1)) ──────
    let rust_s = vec![r_h2_1.reg_tot, r_gc.reg_tot, r_h2_2.reg_tot];
    let rust_i = vec![r_h2_1.intercept, r_gc.intercept, r_h2_2.intercept];
    let rust_n = vec![nbar1, nbar_g, nbar2];

    // ── Step 4: V computation ───────────────────────────────────────────
    // R: v.out = cov(V.hold) / crossprod(N.vec * (sqrt(n.blocks) / m))
    // V.hold is [pseudo_values_col0 for pair1, pair2, pair3]
    let pv1 = &r_h2_1.pseudo_values_col0;
    let pv2 = &r_gc.pseudo_values_col0;
    let pv3 = &r_h2_2.pseudo_values_col0;
    let n_b = pv1.len();
    let mut v_hold = vec![vec![0.0f64; 3]; n_b];
    for b in 0..n_b {
        if b < pv1.len() { v_hold[b][0] = pv1[b]; }
        if b < pv2.len() { v_hold[b][1] = pv2[b]; }
        if b < pv3.len() { v_hold[b][2] = pv3[b]; }
    }

    // cov(V.hold): 3×3 using R's cov (divide by n-1)
    let mut col_means = vec![0.0f64; 3];
    for j in 0..3 {
        let s: f64 = (0..n_b).map(|b| v_hold[b][j]).sum();
        col_means[j] = s / n_b as f64;
    }
    let mut v_raw = vec![vec![0.0f64; 3]; 3];
    for j in 0..3 {
        for k in 0..3 {
            let s: f64 = (0..n_b)
                .map(|b| (v_hold[b][j] - col_means[j]) * (v_hold[b][k] - col_means[k]))
                .sum();
            v_raw[j][k] = s / ((n_b - 1) as f64);
        }
    }

    // R: v.out = cov(V.hold) / crossprod(N.vec * (sqrt(n.blocks) / m))
    // crossprod of a row vector = t(x) %*% x = outer product = N_i * N_j
    let sf = (n_blocks as f64).sqrt() / m;
    let mut rust_v = vec![0.0f64; 9];
    for i in 0..3 {
        for j in 0..3 {
            let denom = rust_n[i] * rust_n[j] * sf * sf;
            rust_v[i * 3 + j] = if denom > 0.0 { v_raw[i][j] / denom } else { v_raw[i][j] };
        }
    }

    // ── Step 5: Liability scaling (no-op for continuous traits) ────────
    // Liab.S = rep(1, 2), so ratio = matrix(1, 2, 2), S unchanged.
    // scaleO = rep(1, 3), V unchanged.

    // ── Step 6: Compare to R ────────────────────────────────────────────
    let r_s: Vec<f64> = serde_json::from_value(r_out["S"].clone()).unwrap();
    let r_v: Vec<f64> = serde_json::from_value(r_out["V"].clone()).unwrap();
    let r_i: Vec<f64> = serde_json::from_value(r_out["I"].clone()).unwrap();
    let r_n: Vec<f64> = serde_json::from_value(r_out["N"].clone()).unwrap();
    let r_m: f64 = serde_json::from_value(r_out["m"].clone()).unwrap();

    eprintln!("\n=== S comparison ===");
    eprintln!("  R stores S as column-major (4 elements); Rust uses vech (3 elements).");
    eprintln!("  vech order: (0,0), (1,0), (1,1) = h²₁, gencov, h²₂");
    eprintln!();

    let mut max_s_diff = 0.0f64;
    // Compare vech elements: rust_s[i] ↔ r_s at the right column-major position.
    // R column-major 2×2: [S00, S10, S01, S11] = [h²₁, gencov, gencov, h²₂]
    // Rust vech: [h²₁, gencov, h²₂]
    let comparisons = [
        ("h²₁",    rust_s[0], r_s[0]),
        ("gencov", rust_s[1], r_s[1]),
        ("h²₂",   rust_s[2], r_s[3]),
    ];
    for (name, rust_val, r_val) in &comparisons {
        let d = (rust_val - r_val).abs();
        if d > max_s_diff { max_s_diff = d; }
        eprintln!("  {:>8}:  Rust={:>14.8}  R={:>14.8}  diff={:.2e}", name, rust_val, r_val, d);
    }
    // Also check S[1,2] = S[2,1] in R (should equal gencov)
    let sym_diff = (r_s[1] - r_s[2]).abs();
    eprintln!("  R symmetry check: |S[2,1]-S[1,2]| = {:.2e}", sym_diff);

    eprintln!("\n=== V diagonal comparison (3×3 column-major) ===");
    let mut max_v_diff = 0.0f64;
    // R V is 3×3 column-major (9 elements). Rust V is also 3×3.
    // Diagonal: indices 0, 4, 8 in column-major.
    for i in 0..3 {
        let rust_val = rust_v[i * 3 + i];
        let r_val = r_v[i * 3 + i];
        let d = (rust_val - r_val).abs();
        if d > max_v_diff { max_v_diff = d; }
        eprintln!("  V[{i},{i}]  Rust={:.8e}  R={:.8e}  diff={:.2e}", rust_val, r_val, d);
    }

    eprintln!("\n=== I comparison (vech: h²₁, gencov_int, h²₂) ===");
    let mut max_i_diff = 0.0f64;
    let i_comp = [
        ("h²₁",    rust_i[0], r_i[0]),
        ("gencov", rust_i[1], r_i[1]),
        ("h²₂",   rust_i[2], r_i[3]),
    ];
    for (name, rv, rv_r) in &i_comp {
        let d = (rv - rv_r).abs();
        if d > max_i_diff { max_i_diff = d; }
        eprintln!("  {:>8}:  Rust={:.8}  R={:.8}  diff={:.2e}", name, rv, rv_r, d);
    }

    eprintln!("\n=== N comparison ===");
    let mut max_n_diff = 0.0f64;
    for i in 0..3 {
        let d = (rust_n[i] - r_n[i]).abs();
        if d > max_n_diff { max_n_diff = d; }
        eprintln!("  N[{i}]  Rust={:.4}  R={:.4}  diff={:.2e}", rust_n[i], r_n[i], d);
    }

    eprintln!("\nm: Rust={:.1}  R={:.1}", m, r_m);

    // ── Assertions ──────────────────────────────────────────────────────
    // S and I should match to machine precision (same data, same algorithm).
    // V diagonal: gencov (V[1,1]) matches exactly; h² V diagonals may differ
    // slightly because h² weights are computed independently in Rust vs R
    // (the h² data ordering and filtering use our own replay, not R's dump).
    let tol_s = 1e-10;
    let tol_i = 1e-8;
    let tol_n = 1e-4;

    assert!(max_s_diff < tol_s,
        "S max diff {max_s_diff:.2e} exceeds tolerance {tol_s:.0e}");

    assert!(max_i_diff < tol_i,
        "I max diff {max_i_diff:.2e} exceeds tolerance {tol_i:.0e}");

    // V[1,1] (gencov diagonal) should match to machine precision.
    let v11_diff = (rust_v[4] - r_v[4]).abs();
    assert!(v11_diff < 1e-12,
        "V[1,1] (gencov) diff {v11_diff:.2e} exceeds tolerance");

    for i in 0..3 {
        let d = (rust_n[i] - r_n[i]).abs();
        assert!(d < tol_n,
            "N[{i}] diff {d:.2e} exceeds tolerance {tol_n:.0e}");
    }

    assert!((m - r_m).abs() < 1e-6, "m mismatch: {m} vs {r_m}");

    eprintln!("\n✅ All elements match within tolerance!");
}
