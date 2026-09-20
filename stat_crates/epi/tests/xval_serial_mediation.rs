//! Cross-validation: survey-weighted serial mediation vs R lm(weights=)
//! hand decomposition (point estimates only; bootstrap CIs are never
//! compared across implementations).
//!
//! Run: `cargo test -p epi --test xval_serial_mediation`

use serde_json::Value;
use std::fs;

use epi::bootstrap::BootstrapDesign;
use epi::mediation_serial::{SerialMediationOptions, mediation_serial};

const TOL: f64 = 1e-4;

struct SerialData {
    x: Vec<f64>,
    m1: Vec<f64>,
    m2: Vec<f64>,
    y: Vec<f64>,
    c1: Vec<f64>,
    w: Vec<f64>,
}

fn load_data() -> SerialData {
    let csv = fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/xval/serial_mediation_data.csv"
    ))
    .expect("serial_mediation_data.csv not found — run gen_serial_mediation_reference.R first");
    let mut d = SerialData {
        x: Vec::new(),
        m1: Vec::new(),
        m2: Vec::new(),
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
        d.m1.push(cols[1].parse().unwrap());
        d.m2.push(cols[2].parse().unwrap());
        d.y.push(cols[3].parse().unwrap());
        d.c1.push(cols[4].parse().unwrap());
        d.w.push(cols[5].parse().unwrap());
    }
    d
}

fn load_reference() -> Value {
    let json = fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/xval/serial_mediation_reference.json"
    ))
    .expect(
        "serial_mediation_reference.json not found — run gen_serial_mediation_reference.R first",
    );
    serde_json::from_str(&json).expect("invalid JSON")
}

fn approx(a: f64, b: f64, tol: f64) -> bool {
    (a - b).abs() <= tol * b.abs().max(1.0)
}

#[test]
fn xval_serial_mediation_vs_r() {
    let d = load_data();
    let r_val = load_reference();
    assert_eq!(d.x.len(), r_val["n"].as_i64().unwrap() as usize);

    let design = BootstrapDesign::new(&d.w).unwrap();
    let opts = SerialMediationOptions {
        n_bootstrap: 0,
        ..Default::default()
    };
    let r = mediation_serial(&d.x, &d.m1, &d.m2, &d.y, &[&d.c1], &design, &opts)
        .expect("serial mediation fit");

    let fields = [
        ("a1", r.a1),
        ("a2", r.a2),
        ("d21", r.d21),
        ("b1", r.b1),
        ("b2", r.b2),
        ("c_prime", r.c_prime),
        ("ie_m1", r.ie_m1),
        ("ie_m2", r.ie_m2),
        ("ie_serial", r.ie_serial),
        ("total_indirect", r.total_indirect),
        ("direct", r.direct),
        ("te", r.te),
        ("prop_mediated", r.prop_mediated),
        ("prop_serial", r.prop_serial),
    ];
    for (name, got) in fields {
        let expected = r_val[name].as_f64().unwrap();
        assert!(
            approx(got, expected, TOL),
            "{name}: got {got:.6}, R {expected:.6}"
        );
    }

    println!("✅ serial mediation: 14 point estimates match R lm(weights=) decomposition");
    println!(
        "  ie_serial = {:.4}, total_indirect = {:.4}, te = {:.4}",
        r.ie_serial, r.total_indirect, r.te
    );
}
