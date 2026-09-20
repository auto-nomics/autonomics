//! Cross-validation: survey-weighted mediation vs R lm(weights=) hand
//! decomposition (point estimates only; bootstrap CIs are never compared
//! across implementations).
//!
//! Run: `cargo test -p epi --test xval_weighted_mediation`

use serde_json::Value;
use std::fs;

use epi::bootstrap::BootstrapDesign;
use epi::mediation_weighted::{MediationWeightedOptions, mediation_weighted};

const TOL: f64 = 1e-4;

struct MedData {
    x: Vec<f64>,
    m: Vec<f64>,
    y: Vec<f64>,
    c1: Vec<f64>,
    c2: Vec<f64>,
    w: Vec<f64>,
}

fn load_data() -> MedData {
    let csv = fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/xval/weighted_mediation_data.csv"
    ))
    .expect("weighted_mediation_data.csv not found — run gen_weighted_mediation_reference.R first");
    let mut d = MedData {
        x: Vec::new(),
        m: Vec::new(),
        y: Vec::new(),
        c1: Vec::new(),
        c2: Vec::new(),
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
        d.y.push(cols[2].parse().unwrap());
        d.c1.push(cols[3].parse().unwrap());
        d.c2.push(cols[4].parse().unwrap());
        d.w.push(cols[5].parse().unwrap());
    }
    d
}

fn load_reference() -> Value {
    let json = fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/xval/weighted_mediation_reference.json"
    ))
    .expect("weighted_mediation_reference.json not found — run gen_weighted_mediation_reference.R first");
    serde_json::from_str(&json).expect("invalid JSON")
}

fn approx(a: f64, b: f64, tol: f64) -> bool {
    (a - b).abs() <= tol * b.abs().max(1.0)
}

fn check(d: &MedData, ref_case: &Value, interaction: bool) {
    let design = BootstrapDesign::new(&d.w).unwrap();
    let opts = MediationWeightedOptions {
        interaction,
        n_bootstrap: 0,
        ..Default::default()
    };
    let r = mediation_weighted(&d.x, &d.m, &d.y, &[&d.c1, &d.c2], &design, &opts)
        .expect("weighted mediation fit");

    let fields = [
        ("alpha_x", r.alpha_x),
        ("beta_x", r.beta_x),
        ("beta_m", r.beta_m),
        ("beta_xm", r.beta_xm),
        ("m_under_control", r.m_under_control),
        ("cde", r.cde),
        ("nde", r.nde),
        ("nie", r.nie),
        ("te", r.te),
        ("prop_mediated", r.prop_mediated),
    ];
    for (name, got) in fields {
        let expected = ref_case[name].as_f64().unwrap();
        assert!(
            approx(got, expected, TOL),
            "{name} (interaction={interaction}): got {got:.6}, R {expected:.6}"
        );
    }

    let label = if interaction {
        "interaction"
    } else {
        "no_interaction"
    };
    println!("✅ {label}: 10 point estimates match R lm(weights=) decomposition");
    println!(
        "  nie = {:.4}, te = {:.4}, prop_mediated = {:.4}",
        r.nie, r.te, r.prop_mediated
    );
}

#[test]
fn xval_weighted_mediation_vs_r() {
    let data = load_data();
    let ref_val = load_reference();
    assert_eq!(data.x.len(), ref_val["n"].as_i64().unwrap() as usize);

    check(&data, &ref_val["no_interaction"], false);
    check(&data, &ref_val["interaction"], true);
}
