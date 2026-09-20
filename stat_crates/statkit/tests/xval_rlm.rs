//! Cross-validation: robust linear regression vs MASS::rlm (Huber + Tukey).
//!
//! Run: `cargo test -p statkit --test xval_rlm`

use serde_json::Value;
use std::fs;

use statkit::regression::{rlm, PsiFunction, RlmOptions};

const TOL: f64 = 1e-4;

struct RlmData {
    x1: Vec<f64>,
    x2: Vec<f64>,
    x3: Vec<f64>,
    y: Vec<f64>,
    w: Vec<f64>,
}

fn load_data() -> RlmData {
    let csv = fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/xval/rlm_data.csv"
    ))
    .expect("rlm_data.csv not found — run gen_rlm_reference.R first");
    let mut x1 = Vec::new();
    let mut x2 = Vec::new();
    let mut x3 = Vec::new();
    let mut y = Vec::new();
    let mut w = Vec::new();

    for (i, line) in csv.lines().enumerate() {
        if i == 0 {
            continue;
        }
        let cols: Vec<&str> = line.split(',').collect();
        if cols.len() < 5 {
            continue;
        }
        x1.push(cols[0].parse().unwrap());
        x2.push(cols[1].parse().unwrap());
        x3.push(cols[2].parse().unwrap());
        y.push(cols[3].parse().unwrap());
        w.push(cols[4].parse().unwrap());
    }
    RlmData { x1, x2, x3, y, w }
}

fn load_reference() -> Value {
    let json = fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/xval/rlm_reference.json"
    ))
    .expect("rlm_reference.json not found — run gen_rlm_reference.R first");
    serde_json::from_str(&json).expect("invalid JSON")
}

fn approx(a: f64, b: f64, tol: f64) -> bool {
    (a - b).abs() <= tol * b.abs().max(1.0)
}

fn check_family(data: &RlmData, ref_family: &Value, psi: PsiFunction, label: &str) {
    let fit = rlm(
        &[&data.x1, &data.x2, &data.x3],
        &data.y,
        &data.w,
        &RlmOptions {
            psi,
            max_iter: 100,
            acc: 1e-10,
            ..RlmOptions::default()
        },
    )
    .expect("rlm fit");

    assert!(fit.converged, "{label} fit should converge");

    let rc = ref_family["coefficients"].as_array().unwrap();
    for i in 0..4 {
        let expected = rc[i].as_f64().unwrap();
        assert!(
            approx(fit.coefficients[i], expected, TOL),
            "{label} coef[{i}]: got {:.6}, R {:.6}",
            fit.coefficients[i],
            expected
        );
    }

    let rs = ref_family["std_errors"].as_array().unwrap();
    for i in 0..4 {
        let expected = rs[i].as_f64().unwrap();
        assert!(
            approx(fit.std_errors[i], expected, TOL),
            "{label} SE[{i}]: got {:.6}, R {:.6}",
            fit.std_errors[i],
            expected
        );
    }

    let r_scale = ref_family["scale"].as_f64().unwrap();
    assert!(
        approx(fit.scale, r_scale, TOL),
        "{label} scale: got {:.6}, R {:.6}",
        fit.scale,
        r_scale
    );

    println!(
        "✅ {label}: 4 coefs, 4 SEs, scale — all match MASS::rlm (wt.method=case, scale.est=MAD)"
    );
    println!("  coefs = {:?}", fit.coefficients);
    println!("  scale = {:.4}", fit.scale);
}

#[test]
fn xval_rlm_vs_mass() {
    let data = load_data();
    let ref_val = load_reference();

    check_family(&data, &ref_val["huber"], PsiFunction::Huber, "Huber");
    check_family(&data, &ref_val["tukey"], PsiFunction::TukeyBisquare, "Tukey");
}
