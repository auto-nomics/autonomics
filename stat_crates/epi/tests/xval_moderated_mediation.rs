//! Cross-validation: survey-weighted moderated mediation vs R
//! lm(weights=) hand decomposition (point estimates only; bootstrap CIs
//! are never compared across implementations).
//!
//! Run: `cargo test -p epi --test xval_moderated_mediation`

use serde_json::Value;
use std::fs;

use epi::bootstrap::BootstrapDesign;
use epi::mediation_moderated::{ModeratedMediationOptions, ModerationStage, mediation_moderated};

const TOL: f64 = 1e-4;

struct ModData {
    x: Vec<f64>,
    m: Vec<f64>,
    wm: Vec<f64>,
    y: Vec<f64>,
    c1: Vec<f64>,
    w: Vec<f64>,
}

fn load_data() -> ModData {
    let csv = fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/xval/moderated_mediation_data.csv"
    ))
    .expect(
        "moderated_mediation_data.csv not found — run gen_moderated_mediation_reference.R first",
    );
    let mut d = ModData {
        x: Vec::new(),
        m: Vec::new(),
        wm: Vec::new(),
        y: Vec::new(),
        c1: Vec::new(),
        w: Vec::new(),
    };
    for (i, line) in csv.lines().enumerate() {
        if i == 0 {
            continue;
        }
        let cols: Vec<&str> = line.split(',').collect();
        if cols.len() < 6 {
            continue;
        }
        d.x.push(cols[0].parse().unwrap());
        d.m.push(cols[1].parse().unwrap());
        d.wm.push(cols[2].parse().unwrap());
        d.y.push(cols[3].parse().unwrap());
        d.c1.push(cols[4].parse().unwrap());
        d.w.push(cols[5].parse().unwrap());
    }
    d
}

fn load_reference() -> Value {
    let json = fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/xval/moderated_mediation_reference.json"
    ))
    .expect("moderated_mediation_reference.json not found — run gen_moderated_mediation_reference.R first");
    serde_json::from_str(&json).expect("invalid JSON")
}

fn approx(a: f64, b: f64, tol: f64) -> bool {
    (a - b).abs() <= tol * b.abs().max(1.0)
}

fn grid_from_reference(r_val: &Value) -> Vec<f64> {
    r_val["w_grid"]
        .as_array()
        .expect("w_grid array")
        .iter()
        .map(|v| v.as_f64().unwrap())
        .collect()
}

/// Compare scalar fields and per-grid-point conditional quantities.
fn check_scalar(got: f64, expected: f64, name: &str) {
    assert!(
        approx(got, expected, TOL),
        "{name}: got {got:.6}, R {expected:.6}"
    );
}

#[test]
fn xval_moderated_mediation_vs_r() {
    let d = load_data();
    let r_val = load_reference();
    assert_eq!(d.x.len(), r_val["n"].as_i64().unwrap() as usize);
    let w_grid = grid_from_reference(&r_val);

    let design = BootstrapDesign::new(&d.w).unwrap();

    // ── First stage: X:W in the mediator model ─────────────────────────
    {
        let opts = ModeratedMediationOptions {
            stage: ModerationStage::First,
            w_grid: Some(w_grid.clone()),
            n_bootstrap: 0,
            ..Default::default()
        };
        let r = mediation_moderated(&d.x, &d.m, &d.wm, &d.y, &[&d.c1], &design, &opts)
            .expect("first-stage moderated mediation fit");

        let f = &r_val["first"];
        check_scalar(r.a_x, f["a_x"].as_f64().unwrap(), "first a_x");
        check_scalar(r.a_xw, f["a_xw"].as_f64().unwrap(), "first a_xw");
        check_scalar(r.b_m, f["b_m"].as_f64().unwrap(), "first b_m");
        check_scalar(r.c_x, f["c_x"].as_f64().unwrap(), "first c_x");
        check_scalar(
            r.index_first_stage,
            f["index_first_stage"].as_f64().unwrap(),
            "first index",
        );
        for j in 0..w_grid.len() {
            check_scalar(
                r.conditional[j].a_path,
                f["a_path"][j].as_f64().unwrap(),
                "first a_path",
            );
            check_scalar(
                r.conditional[j].b_path,
                f["b_path"][j].as_f64().unwrap(),
                "first b_path",
            );
            check_scalar(
                r.conditional[j].indirect,
                f["indirect"][j].as_f64().unwrap(),
                "first indirect",
            );
            check_scalar(
                r.conditional_direct[j].1,
                f["direct"][j].as_f64().unwrap(),
                "first direct",
            );
        }
        println!("✅ first stage: paths + index + 3 conditional effects match R");
    }

    // ── Second stage: M:W in the outcome model ────────────────────────
    {
        let opts = ModeratedMediationOptions {
            stage: ModerationStage::Second,
            w_grid: Some(w_grid.clone()),
            n_bootstrap: 0,
            ..Default::default()
        };
        let r = mediation_moderated(&d.x, &d.m, &d.wm, &d.y, &[&d.c1], &design, &opts)
            .expect("second-stage moderated mediation fit");

        let s = &r_val["second"];
        check_scalar(r.a_x, s["a_x"].as_f64().unwrap(), "second a_x");
        check_scalar(r.b_m, s["b_m"].as_f64().unwrap(), "second b_m");
        check_scalar(r.b_mw, s["b_mw"].as_f64().unwrap(), "second b_mw");
        check_scalar(r.c_x, s["c_x"].as_f64().unwrap(), "second c_x");
        check_scalar(
            r.index_second_stage,
            s["index_second_stage"].as_f64().unwrap(),
            "second index",
        );
        for j in 0..w_grid.len() {
            check_scalar(
                r.conditional[j].a_path,
                s["a_path"][j].as_f64().unwrap(),
                "second a_path",
            );
            check_scalar(
                r.conditional[j].b_path,
                s["b_path"][j].as_f64().unwrap(),
                "second b_path",
            );
            check_scalar(
                r.conditional[j].indirect,
                s["indirect"][j].as_f64().unwrap(),
                "second indirect",
            );
            check_scalar(
                r.conditional_direct[j].1,
                s["direct"][j].as_f64().unwrap(),
                "second direct",
            );
        }
        println!("✅ second stage: paths + index + 3 conditional effects match R");
    }
}
