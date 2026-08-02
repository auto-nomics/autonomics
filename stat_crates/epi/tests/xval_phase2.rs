//! Cross-validation: Phase 2 methods (IPTW + CLPM) vs R reference.
//!
//! Run: `cargo test -p epi --test xval_phase2`

use serde_json::Value;
use std::fs;

const TOL: f64 = 1e-4;

fn load_csv(path: &str) -> Vec<Vec<f64>> {
    let content = fs::read_to_string(path).expect("data file not found");
    let mut rows = Vec::new();
    for (i, line) in content.lines().enumerate() {
        if i == 0 {
            continue;
        }
        let cols: Vec<f64> = line.split(",").filter_map(|c| c.parse().ok()).collect();
        if !cols.is_empty() {
            rows.push(cols);
        }
    }
    rows
}

fn load_reference() -> Value {
    let json = fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/xval/phase2_reference.json"
    ))
    .expect("phase2_reference.json not found");
    serde_json::from_str(&json).expect("invalid JSON")
}

fn approx(a: f64, b: f64, tol: f64) -> bool {
    (a - b).abs() <= tol * b.abs().max(1.0)
}

// ── IPTW vs R (glm + weighted lm) ──────────────────────────────────────────

#[test]
fn xval_iptw_vs_r() {
    let base = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/xval/iptw_data.csv");
    let rows = load_csv(base);
    let treatment: Vec<f64> = rows.iter().map(|r| r[0]).collect();
    let outcome: Vec<f64> = rows.iter().map(|r| r[1]).collect();
    let c1: Vec<f64> = rows.iter().map(|r| r[2]).collect();
    let c2: Vec<f64> = rows.iter().map(|r| r[3]).collect();

    let opts = epi::causal::IptwOptions {
        n_bootstrap: 0,
        ..Default::default()
    };
    let result = epi::causal::iptw(&treatment, &outcome, &[&c1, &c2], &opts).unwrap();

    let ref_val = load_reference();
    let r_ate = ref_val["iptw"]["ate"].as_f64().unwrap();

    // IPTW ATE should match R's stabilized-weight ATE closely.
    // Both use logistic PS + stabilized weights + WLS, but may differ slightly
    // due to trim and convergence. Use a tolerance of 0.05.
    assert!(
        approx(result.ate, r_ate, 0.05),
        "IPTW ATE: got {:.4}, R {:.4}",
        result.ate,
        r_ate
    );

    println!("✅ IPTW: ATE={:.4} (R {:.4})", result.ate, r_ate);
}

// ── CLPM vs R lm() ──────────────────────────────────────────────────────────

#[test]
fn xval_clpm_vs_r_lm() {
    let base = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/xval/clpm_data.csv");
    let rows = load_csv(base);
    let x1: Vec<f64> = rows.iter().map(|r| r[0]).collect();
    let y1: Vec<f64> = rows.iter().map(|r| r[1]).collect();
    let x2: Vec<f64> = rows.iter().map(|r| r[2]).collect();
    let y2: Vec<f64> = rows.iter().map(|r| r[3]).collect();

    let result = epi::clpm::clpm(&x1, &y1, &x2, &y2, 0, 0).unwrap();

    let ref_val = load_reference();
    let r_ar_y = ref_val["clpm"]["ar_y"].as_f64().unwrap();
    let r_cross_xy = ref_val["clpm"]["cross_xy"].as_f64().unwrap();
    let r_ar_x = ref_val["clpm"]["ar_x"].as_f64().unwrap();
    let r_cross_yx = ref_val["clpm"]["cross_yx"].as_f64().unwrap();

    assert!(
        approx(result.ar_y, r_ar_y, TOL),
        "AR(Y): got {:.6}, R {:.6}",
        result.ar_y,
        r_ar_y
    );
    assert!(
        approx(result.cross_xy, r_cross_xy, TOL),
        "cross(X→Y): got {:.6}, R {:.6}",
        result.cross_xy,
        r_cross_xy
    );
    assert!(
        approx(result.ar_x, r_ar_x, TOL),
        "AR(X): got {:.6}, R {:.6}",
        result.ar_x,
        r_ar_x
    );
    assert!(
        approx(result.cross_yx, r_cross_yx, TOL),
        "cross(Y→X): got {:.6}, R {:.6}",
        result.cross_yx,
        r_cross_yx
    );

    println!(
        "✅ CLPM: AR(Y)={:.4} (R {:.4}), cross(X→Y)={:.4} (R {:.4})",
        result.ar_y, r_ar_y, result.cross_xy, r_cross_xy
    );
    println!(
        "  AR(X)={:.4} (R {:.4}), cross(Y→X)={:.4} (R {:.4})",
        result.ar_x, r_ar_x, result.cross_yx, r_cross_yx
    );
}
