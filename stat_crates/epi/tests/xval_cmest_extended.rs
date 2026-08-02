//! Cross-validation: Extended cmest variants vs R (CMAverse source formulas).
//! Tests: multi-mediator, binary outcome, binary mediator, weighting, g-formula.
//!
//! Run: `cargo test -p epi --test xval_cmest_extended -- --nocapture`

use serde_json::Value;
use std::fs;

const TOL: f64 = 0.02; // Looser for logistic-based variants

fn load_json(path: &str) -> Value {
    let json = fs::read_to_string(path).expect("JSON not found");
    serde_json::from_str(&json).expect("invalid JSON")
}

fn load_csv(path: &str) -> Vec<Vec<f64>> {
    let content = fs::read_to_string(path).expect("data not found");
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

fn approx(a: f64, b: f64, tol: f64) -> bool {
    (a - b).abs() <= tol * b.abs().max(1.0)
}

macro_rules! check {
    ($name:expr, $ours:expr, $r:expr, $tol:expr) => {
        assert!(
            approx($ours, $r, $tol),
            "{}: ours {:.6}, R {:.6}",
            $name,
            $ours,
            $r
        );
    };
}

// ═══════════════════════════════════════════════════════════════════════════
// 1. Multi-mediator
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn xval_multi_mediator() {
    let rows = load_csv(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/xval/cmest_multi_data.csv"
    ));
    let x: Vec<f64> = rows.iter().map(|r| r[0]).collect();
    let m1: Vec<f64> = rows.iter().map(|r| r[1]).collect();
    let m2: Vec<f64> = rows.iter().map(|r| r[2]).collect();
    let y: Vec<f64> = rows.iter().map(|r| r[3]).collect();
    let c: Vec<f64> = rows.iter().map(|r| r[4]).collect();

    let opts = epi::cmest::CmestOptions {
        interaction: false,
        n_bootstrap: 0,
        ..Default::default()
    };
    let r = epi::cmest::cmest_multi(&x, &[&m1, &m2], &y, &[&c], &opts).unwrap();
    let ref_val = load_json(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/xval/cmest_extended_reference.json"
    ));

    let tol = 1e-4;
    check!(
        "Multi NDE",
        r.nde,
        ref_val["multi"]["nde"].as_f64().unwrap(),
        tol
    );
    check!(
        "Multi NIE",
        r.nie,
        ref_val["multi"]["nie"].as_f64().unwrap(),
        tol
    );
    check!(
        "Multi TE",
        r.te,
        ref_val["multi"]["te"].as_f64().unwrap(),
        tol
    );

    println!(
        "✅ Multi-mediator: NDE={:.4}, NIE={:.4}, TE={:.4} (R {:.4}, {:.4}, {:.4})",
        r.nde,
        r.nie,
        r.te,
        ref_val["multi"]["nde"].as_f64().unwrap(),
        ref_val["multi"]["nie"].as_f64().unwrap(),
        ref_val["multi"]["te"].as_f64().unwrap()
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// 2. Binary outcome (OR scale)
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn xval_binary_outcome() {
    let rows = load_csv(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/xval/cmest_biny_data.csv"
    ));
    let x: Vec<f64> = rows.iter().map(|r| r[0]).collect();
    let m: Vec<f64> = rows.iter().map(|r| r[1]).collect();
    let y: Vec<f64> = rows.iter().map(|r| r[2]).collect();

    let opts = epi::cmest::CmestOptions {
        interaction: false,
        n_bootstrap: 0,
        ..Default::default()
    };
    let r = epi::cmest::cmest_binary_y(&x, &m, &y, &[], &opts).unwrap();
    let ref_val = load_json(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/xval/cmest_extended_reference.json"
    ));

    check!(
        "BinY NDE(OR)",
        r.nde,
        ref_val["biny"]["nde"].as_f64().unwrap(),
        TOL
    );
    check!(
        "BinY NIE(OR)",
        r.nie,
        ref_val["biny"]["nie"].as_f64().unwrap(),
        TOL
    );
    check!(
        "BinY TE(OR)",
        r.te,
        ref_val["biny"]["te"].as_f64().unwrap(),
        TOL
    );

    println!(
        "✅ Binary outcome (OR scale): NDE={:.4}, NIE={:.4}, TE={:.4} (R {:.4}, {:.4}, {:.4})",
        r.nde,
        r.nie,
        r.te,
        ref_val["biny"]["nde"].as_f64().unwrap(),
        ref_val["biny"]["nie"].as_f64().unwrap(),
        ref_val["biny"]["te"].as_f64().unwrap()
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// 3. Binary mediator
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn xval_binary_mediator() {
    let rows = load_csv(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/xval/cmest_binm_data.csv"
    ));
    let x: Vec<f64> = rows.iter().map(|r| r[0]).collect();
    let m: Vec<f64> = rows.iter().map(|r| r[1]).collect();
    let y: Vec<f64> = rows.iter().map(|r| r[2]).collect();

    let opts = epi::cmest::CmestOptions {
        interaction: false,
        ..Default::default()
    };
    let r = epi::cmest::cmest_binary_m(&x, &m, &y, &[], &opts).unwrap();
    let ref_val = load_json(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/xval/cmest_extended_reference.json"
    ));

    check!(
        "BinM NDE",
        r.nde,
        ref_val["binm"]["nde"].as_f64().unwrap(),
        TOL
    );
    check!(
        "BinM NIE",
        r.nie,
        ref_val["binm"]["nie"].as_f64().unwrap(),
        TOL
    );
    check!(
        "BinM TE",
        r.te,
        ref_val["binm"]["te"].as_f64().unwrap(),
        TOL
    );

    println!(
        "✅ Binary mediator: NDE={:.4}, NIE={:.4}, TE={:.4} (R {:.4}, {:.4}, {:.4})",
        r.nde,
        r.nie,
        r.te,
        ref_val["binm"]["nde"].as_f64().unwrap(),
        ref_val["binm"]["nie"].as_f64().unwrap(),
        ref_val["binm"]["te"].as_f64().unwrap()
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// 4. Weighting-based
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn xval_weighting() {
    let rows = load_csv(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/xval/cmest_wb_gf_data.csv"
    ));
    let x: Vec<f64> = rows.iter().map(|r| r[0]).collect();
    let m: Vec<f64> = rows.iter().map(|r| r[1]).collect();
    let y: Vec<f64> = rows.iter().map(|r| r[2]).collect();
    let c: Vec<f64> = rows.iter().map(|r| r[3]).collect();

    let opts = epi::cmest::CmestOptions {
        interaction: false,
        ..Default::default()
    };
    let r = epi::cmest::cmest_weighting(&x, &m, &y, &[&c], &opts).unwrap();
    let ref_val = load_json(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/xval/cmest_extended_reference.json"
    ));

    check!("WB TE", r.te, ref_val["wb"]["te"].as_f64().unwrap(), TOL);

    println!(
        "✅ Weighting: TE={:.4} (R {:.4})",
        r.te,
        ref_val["wb"]["te"].as_f64().unwrap()
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// 5. g-formula
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn xval_gformula() {
    let rows = load_csv(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/xval/cmest_wb_gf_data.csv"
    ));
    let x: Vec<f64> = rows.iter().map(|r| r[0]).collect();
    let m: Vec<f64> = rows.iter().map(|r| r[1]).collect();
    let y: Vec<f64> = rows.iter().map(|r| r[2]).collect();
    let c: Vec<f64> = rows.iter().map(|r| r[3]).collect();

    let opts = epi::cmest::CmestOptions {
        interaction: false,
        ..Default::default()
    };
    let r = epi::cmest::cmest_gformula(&x, &m, &y, &[&c], &opts).unwrap();
    let ref_val = load_json(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/xval/cmest_extended_reference.json"
    ));

    check!("GF NDE", r.nde, ref_val["gf"]["nde"].as_f64().unwrap(), TOL);
    check!("GF TE", r.te, ref_val["gf"]["te"].as_f64().unwrap(), TOL);

    println!(
        "✅ g-formula: NDE={:.4}, TE={:.4} (R {:.4}, {:.4})",
        r.nde,
        r.te,
        ref_val["gf"]["nde"].as_f64().unwrap(),
        ref_val["gf"]["te"].as_f64().unwrap()
    );
}
