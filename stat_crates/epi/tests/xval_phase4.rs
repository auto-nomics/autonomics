//! Cross-validation: Phase 4 Competing Risk (CIF) vs R cmprsk::cuminc.
//!
//! Run: `cargo test -p epi --test xval_phase4 -- --nocapture`

use serde_json::Value;
use std::fs;

fn load_reference() -> Value {
    let json = fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/xval/phase4_reference.json"))
        .expect("phase4_reference.json not found");
    serde_json::from_str(&json).expect("invalid JSON")
}

fn load_csv(path: &str) -> Vec<Vec<f64>> {
    let content = fs::read_to_string(path).expect("data file not found");
    let mut rows = Vec::new();
    for (i, line) in content.lines().enumerate() {
        if i == 0 { continue; }
        let cols: Vec<f64> = line.split(",").filter_map(|c| c.parse().ok()).collect();
        if !cols.is_empty() { rows.push(cols); }
    }
    rows
}

#[test]
fn xval_cif_vs_r_cuminc() {
    let rows = load_csv(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/xval/competing_risk_data.csv"));
    let time: Vec<f64> = rows.iter().map(|r| r[0]).collect();
    let event: Vec<f64> = rows.iter().map(|r| r[1]).collect();

    let result = epi::competing_risk::cumulative_incidence(&time, &event)
        .expect("CIF estimation");

    let ref_val = load_reference();

    // ── Verify causes are detected ──────────────────────────────────────
    let r_n1 = ref_val["n_cause1"].as_i64().unwrap() as usize;
    let r_n2 = ref_val["n_cause2"].as_i64().unwrap() as usize;
    assert!(result.causes.contains(&1), "Should detect cause 1");
    assert!(result.causes.contains(&2), "Should detect cause 2");

    // ── Verify event counts ─────────────────────────────────────────────
    let cause1_idx = result.causes.iter().position(|&c| c == 1).unwrap();
    let cause2_idx = result.causes.iter().position(|&c| c == 2).unwrap();
    assert_eq!(result.n_events_per_cause[cause1_idx], r_n1, "Cause 1 event count");
    assert_eq!(result.n_events_per_cause[cause2_idx], r_n2, "Cause 2 event count");

    // ── Compare final CIF values ────────────────────────────────────────
    // Our CIF: cif[cause_idx][last_time]. R: cif1_last / cif2_last.
    let our_cif1_last = *result.cif[cause1_idx].last().unwrap();
    let our_cif2_last = *result.cif[cause2_idx].last().unwrap();
    let r_cif1_last = ref_val["cif1_last"].as_f64().unwrap();
    let r_cif2_last = ref_val["cif2_last"].as_f64().unwrap();

    assert!(
        (our_cif1_last - r_cif1_last).abs() < 0.01,
        "CIF1 last: ours {:.6}, R {:.6}", our_cif1_last, r_cif1_last
    );
    assert!(
        (our_cif2_last - r_cif2_last).abs() < 0.01,
        "CIF2 last: ours {:.6}, R {:.6}", our_cif2_last, r_cif2_last
    );

    // ── CIF1 + CIF2 + S(last) ≤ 1 ──────────────────────────────────────
    let s_last = *result.survival.last().unwrap();
    let total = our_cif1_last + our_cif2_last + s_last;
    assert!(
        total <= 1.0 + 1e-10,
        "CIF1+CIF2+S = {total} should ≤ 1"
    );

    // ── Point-by-point comparison at matching times ─────────────────────
    // R includes time=0 and times of all events; our impl includes event times only.
    // Compare at R's event-time points by linear interpolation.
    let r_times = ref_val["cif1_times"].as_array().unwrap();
    let r_cif1 = ref_val["cif1"].as_array().unwrap();

    // Find matching time points (skip time=0 which our impl doesn't include).
    let mut max_diff = 0.0_f64;
    let mut compared = 0;
    for (i, rt) in r_times.iter().enumerate() {
        let rt_val = rt.as_f64().unwrap();
        if rt_val < 1e-10 { continue; } // skip t=0

        // Find our closest time point ≤ rt_val.
        if let Some(j) = result.times.iter().position(|&t| (t - rt_val).abs() < 0.01) {
            let our_val = result.cif[cause1_idx][j];
            let r_val = r_cif1[i].as_f64().unwrap();
            let diff = (our_val - r_val).abs();
            if diff > max_diff { max_diff = diff; }
            compared += 1;
        }
    }

    assert!(compared > 50, "Should compare at least 50 time points, got {compared}");
    assert!(
        max_diff < 0.01,
        "CIF1 max pointwise diff = {:.6} across {compared} points", max_diff
    );

    println!("✅ CIF vs R cuminc: CIF1_last={:.4} (R {:.4}), CIF2_last={:.4} (R {:.4})",
        our_cif1_last, r_cif1_last, our_cif2_last, r_cif2_last);
    println!("   Pointwise comparison: {} points, max diff = {:.6}", compared, max_diff);
    println!("   CIF1+CIF2+S(last) = {:.4}+{:.4}+{:.4} = {:.4}",
        our_cif1_last, our_cif2_last, s_last, total);
}

// ── MultiState vs CIF cross-check ──────────────────────────────────────────
// The Aalen-Johansen P(0→k) should match the CIF for cause k.

#[test]
fn xval_multistate_matches_cif() {
    let rows = load_csv(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/xval/competing_risk_data.csv"));
    let time: Vec<f64> = rows.iter().map(|r| r[0]).collect();
    let event: Vec<f64> = rows.iter().map(|r| r[1]).collect();

    // CIF estimation.
    let cif = epi::competing_risk::cumulative_incidence(&time, &event)
        .expect("CIF");

    // Multi-state: all subjects start in state 0, transition to state k (cause k).
    let n = time.len();
    let from: Vec<u64> = vec![0; n];
    let to: Vec<u64> = event.iter().map(|&e| e as u64).collect(); // 0=censored→0, 1→1, 2→2

    let ms = epi::multistate::multistate(&time, &from, &to, 3, None)
        .expect("multistate");

    // Compare final P(0→1) with CIF1_last, P(0→2) with CIF2_last.
    let cause1_idx = cif.causes.iter().position(|&c| c == 1).unwrap();
    let cause2_idx = cif.causes.iter().position(|&c| c == 2).unwrap();
    let cif1_last = *cif.cif[cause1_idx].last().unwrap();
    let cif2_last = *cif.cif[cause2_idx].last().unwrap();

    let p_last = ms.p_matrices.last().unwrap();
    let p01 = p_last[0][1]; // P(0→1)
    let p02 = p_last[0][2]; // P(0→2)

    assert!(
        (p01 - cif1_last).abs() < 0.01,
        "P(0→1)={:.6} vs CIF1_last={:.6}", p01, cif1_last
    );
    assert!(
        (p02 - cif2_last).abs() < 0.01,
        "P(0→2)={:.6} vs CIF2_last={:.6}", p02, cif2_last
    );

    // Row stochastic: P(0→0) + P(0→1) + P(0→2) = 1.
    let row_sum = p_last[0][0] + p01 + p02;
    assert!((row_sum - 1.0).abs() < 1e-10, "Row 0 sum = {row_sum}");

    println!("✅ MultiState vs CIF: P(0→1)={:.4} (CIF1 {:.4}), P(0→2)={:.4} (CIF2 {:.4})",
        p01, cif1_last, p02, cif2_last);
    println!("   P(0→0)={:.4}, row sum={:.4}", p_last[0][0], row_sum);
}
