//! Cross-validation: causal mediation analysis vs R lm() two-model approach.
//!
//! Run: `cargo test -p epi --test xval_mediation`

use serde_json::Value;
use std::fs;

const TOL: f64 = 1e-4;

struct MedData {
    x: Vec<f64>,
    m: Vec<f64>,
    y: Vec<f64>,
    c1: Vec<f64>,
}

fn load_data() -> MedData {
    let csv = fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/xval/mediation_data.csv"
    ))
    .expect("mediation_data.csv not found");
    let mut x = Vec::new();
    let mut m = Vec::new();
    let mut y = Vec::new();
    let mut c1 = Vec::new();
    for (i, line) in csv.lines().enumerate() {
        if i == 0 {
            continue;
        }
        let cols: Vec<&str> = line.split(',').collect();
        if cols.len() < 4 {
            continue;
        }
        x.push(cols[0].parse().unwrap());
        m.push(cols[1].parse().unwrap());
        y.push(cols[2].parse().unwrap());
        c1.push(cols[3].parse().unwrap());
    }
    MedData { x, m, y, c1 }
}

fn load_reference() -> Value {
    let json = fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/xval/mediation_reference.json"
    ))
    .expect("mediation_reference.json not found");
    serde_json::from_str(&json).expect("invalid JSON")
}

fn approx(a: f64, b: f64, tol: f64) -> bool {
    (a - b).abs() <= tol * b.abs().max(1.0)
}

#[test]
fn xval_mediation_vs_r_lm() {
    let data = load_data();
    let ref_val = load_reference();

    // No bootstrap for the point-estimate comparison (we compare the exact
    // decomposition from the same data).
    let opts = epi::mediation::MediationOptions {
        n_bootstrap: 0, // skip bootstrap — only compare point estimates
        ..Default::default()
    };
    let result = epi::mediation::mediation(&data.x, &data.m, &data.y, &[&data.c1], false, &opts)
        .expect("mediation fit");

    // Mediator model α₁ (X → M).
    let r_alpha_1 = ref_val["alpha_1"].as_f64().unwrap();
    assert!(
        approx(result.alpha_x, r_alpha_1, TOL),
        "α₁: got {:.6}, R {:.6}",
        result.alpha_x,
        r_alpha_1
    );

    // Outcome model β₁ (direct X → Y).
    let r_beta_1 = ref_val["beta_1"].as_f64().unwrap();
    assert!(
        approx(result.beta_x, r_beta_1, TOL),
        "β₁: got {:.6}, R {:.6}",
        result.beta_x,
        r_beta_1
    );

    // Outcome model β₂ (M → Y).
    let r_beta_2 = ref_val["beta_2"].as_f64().unwrap();
    assert!(
        approx(result.beta_m, r_beta_2, TOL),
        "β₂: got {:.6}, R {:.6}",
        result.beta_m,
        r_beta_2
    );

    // NDE.
    let r_nde = ref_val["nde"].as_f64().unwrap();
    assert!(
        approx(result.nde, r_nde, TOL),
        "NDE: got {:.6}, R {:.6}",
        result.nde,
        r_nde
    );

    // NIE.
    let r_nie = ref_val["nie"].as_f64().unwrap();
    assert!(
        approx(result.nie, r_nie, TOL),
        "NIE: got {:.6}, R {:.6}",
        result.nie,
        r_nie
    );

    // TE.
    let r_te = ref_val["te"].as_f64().unwrap();
    assert!(
        approx(result.te, r_te, TOL),
        "TE: got {:.6}, R {:.6}",
        result.te,
        r_te
    );

    // Proportion mediated.
    let r_pm = ref_val["prop_mediated"].as_f64().unwrap();
    assert!(
        approx(result.prop_mediated, r_pm, TOL),
        "Prop mediated: got {:.6}, R {:.6}",
        result.prop_mediated,
        r_pm
    );

    println!(
        "✅ Mediation: α₁={:.4}, β₁={:.4}, β₂={:.4}",
        result.alpha_x, result.beta_x, result.beta_m
    );
    println!(
        "  NDE={:.4} (R {:.4}), NIE={:.4} (R {:.4}), TE={:.4} (R {:.4})",
        result.nde, r_nde, result.nie, r_nie, result.te, r_te
    );
    println!(
        "  Prop mediated={:.4} (R {:.4})",
        result.prop_mediated, r_pm
    );
}
