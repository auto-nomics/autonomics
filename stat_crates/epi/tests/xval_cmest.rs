//! Cross-validation: CMAverse-compatible cmest vs R.
//! Two levels of validation:
//! 1. Manual Valeri/VanderWeele formulas (no interaction)
//! 2. CMAverse source-level formulas from est_rb.R (with interaction + covariate means)
//!
//! Run: `cargo test -p epi --test xval_cmest -- --nocapture`

use serde_json::Value;
use std::fs;

const TOL: f64 = 1e-4;

fn load_json(path: &str) -> Value {
    let json = fs::read_to_string(path).expect("reference JSON not found");
    serde_json::from_str(&json).expect("invalid JSON")
}

fn approx(a: f64, b: f64, tol: f64) -> bool {
    (a - b).abs() <= tol * b.abs().max(1.0)
}

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

// ═══════════════════════════════════════════════════════════════════════════
// 1. No-interaction case: vs manual R formulas (Valeri/VanderWeele rb)
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn xval_cmest_no_interaction_vs_r() {
    let rows = load_csv(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/xval/cmest_data.csv"
    ));
    let x: Vec<f64> = rows.iter().map(|r| r[0]).collect();
    let m: Vec<f64> = rows.iter().map(|r| r[1]).collect();
    let y: Vec<f64> = rows.iter().map(|r| r[2]).collect();
    let c: Vec<f64> = rows.iter().map(|r| r[3]).collect();

    let opts = epi::cmest::CmestOptions {
        interaction: false,
        n_bootstrap: 0,
        ..Default::default()
    };
    let result = epi::cmest::cmest(&x, &m, &y, &[&c], &opts).unwrap();

    let ref_val = load_json(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/xval/cmest_reference.json"
    ));
    let checks = [
        ("CDE", result.cde, ref_val["cde"].as_f64().unwrap()),
        ("NDE", result.nde, ref_val["nde"].as_f64().unwrap()),
        ("NIE", result.nie, ref_val["nie"].as_f64().unwrap()),
        ("TE", result.te, ref_val["te"].as_f64().unwrap()),
        (
            "prop_mediated",
            result.prop_mediated,
            ref_val["prop_mediated"].as_f64().unwrap(),
        ),
        (
            "prop_eliminated",
            result.prop_eliminated,
            ref_val["prop_eliminated"].as_f64().unwrap(),
        ),
    ];

    for (name, ours, theirs) in &checks {
        assert!(
            approx(*ours, *theirs, TOL),
            "{}: got {:.6}, R {:.6}",
            name,
            ours,
            theirs
        );
    }
    println!(
        "✅ No-interaction case: all 6 effects match R (tol={})",
        TOL
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// 2. Interaction case: vs CMAverse source-level formulas (est_rb.R)
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn xval_cmest_with_interaction_vs_cmaverse_source() {
    let rows = load_csv(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/xval/cmest_data.csv"
    ));
    let x: Vec<f64> = rows.iter().map(|r| r[0]).collect();
    let m: Vec<f64> = rows.iter().map(|r| r[1]).collect();
    let y: Vec<f64> = rows.iter().map(|r| r[2]).collect();
    let c: Vec<f64> = rows.iter().map(|r| r[3]).collect();

    let opts = epi::cmest::CmestOptions {
        interaction: true, // X×M interaction in outcome model
        cde_m: 0.0,        // CDE at m=0 (matches CMAverse mstar=0)
        n_bootstrap: 0,
        ..Default::default()
    };
    let result = epi::cmest::cmest(&x, &m, &y, &[&c], &opts).unwrap();

    // CMAverse source-level reference (from gen_cmest_cmaverse.R).
    let ref_val = load_json(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/xval/cmest_cmaverse_reference.json"
    ));

    let r_cde = ref_val["cde"].as_f64().unwrap();
    let r_nde = ref_val["nde"].as_f64().unwrap();
    let r_nie = ref_val["nie"].as_f64().unwrap();
    let r_te = ref_val["te"].as_f64().unwrap();
    let r_pm = ref_val["prop_mediated"].as_f64().unwrap();
    let r_pe = ref_val["prop_eliminated"].as_f64().unwrap();

    let checks = [
        ("CDE", result.cde, r_cde),
        ("NDE(pnde)", result.nde, r_nde),
        ("NIE(tnie)", result.nie, r_nie),
        ("TE", result.te, r_te),
        ("prop_mediated", result.prop_mediated, r_pm),
        ("prop_eliminated", result.prop_eliminated, r_pe),
    ];

    for (name, ours, theirs) in &checks {
        assert!(
            approx(*ours, *theirs, TOL),
            "{}: got {:.6}, CMAverse source {:.6}",
            name,
            ours,
            theirs
        );
    }

    println!(
        "✅ Interaction case: all 6 effects match CMAverse est_rb.R source (tol={})",
        TOL
    );
    for (name, ours, theirs) in &checks {
        println!("   {} = {:.6} (CMAverse {:.6})", name, ours, theirs);
    }
}
