//! Cross-validation: Cox PH regression vs R survival::coxph (Breslow ties).
//!
//! Run: `cargo test -p statkit --test xval_cox`

use serde_json::Value;
use std::fs;

const TOL_COEF: f64 = 1e-4;
const TOL_LL: f64 = 1e-2;
const TOL_C: f64 = 1e-2;

struct CoxData {
    x1: Vec<f64>,
    x2: Vec<f64>,
    x3: Vec<f64>,
    time: Vec<f64>,
    event: Vec<f64>,
}

fn load_data() -> CoxData {
    let csv = fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/xval/cox_data.csv"
    ))
    .expect("cox_data.csv not found — run gen_cox_reference.R first");
    let mut x1 = Vec::new();
    let mut x2 = Vec::new();
    let mut x3 = Vec::new();
    let mut time = Vec::new();
    let mut event = Vec::new();

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
        time.push(cols[3].parse().unwrap());
        event.push(cols[4].parse().unwrap());
    }
    CoxData {
        x1,
        x2,
        x3,
        time,
        event,
    }
}

fn load_reference() -> Value {
    let json = fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/xval/cox_reference.json"
    ))
    .expect("cox_reference.json not found — run gen_cox_reference.R first");
    serde_json::from_str(&json).expect("invalid JSON")
}

fn approx(a: f64, b: f64, tol: f64) -> bool {
    (a - b).abs() <= tol * b.abs().max(1.0)
}

#[test]
fn xval_cox_vs_r_coxph() {
    let data = load_data();
    let ref_val = load_reference();

    let fit = statkit::regression::cox(&data.time, &data.event, &[&data.x1, &data.x2, &data.x3])
        .expect("Cox fit");

    assert!(fit.converged, "Cox model should converge");

    // ── Coefficients ────────────────────────────────────────────────────
    let rc = ref_val["coefficients"].as_array().unwrap();
    for i in 0..3 {
        let expected = rc[i].as_f64().unwrap();
        assert!(
            approx(fit.coefficients[i], expected, TOL_COEF),
            "Cox coef[{i}]: got {:.6}, R {:.6}",
            fit.coefficients[i],
            expected
        );
    }

    // ── Standard errors ────────────────────────────────────────────────
    let rs = ref_val["std_errors"].as_array().unwrap();
    for i in 0..3 {
        let expected = rs[i].as_f64().unwrap();
        assert!(
            approx(fit.std_errors[i], expected, TOL_COEF),
            "Cox SE[{i}]: got {:.6}, R {:.6}",
            fit.std_errors[i],
            expected
        );
    }

    // ── Hazard ratios ──────────────────────────────────────────────────
    let rhr = ref_val["hazard_ratios"].as_array().unwrap();
    for i in 0..3 {
        let expected = rhr[i].as_f64().unwrap();
        assert!(
            approx(fit.hazard_ratios[i], expected, TOL_COEF),
            "Cox HR[{i}]: got {:.6}, R {:.6}",
            fit.hazard_ratios[i],
            expected
        );
    }

    // ── HR confidence intervals ────────────────────────────────────────
    let rlo = ref_val["hr_ci_lower"].as_array().unwrap();
    let rhi = ref_val["hr_ci_upper"].as_array().unwrap();
    for i in 0..3 {
        assert!(
            approx(fit.hr_ci_lower[i], rlo[i].as_f64().unwrap(), TOL_COEF),
            "Cox HR lower[{i}]: got {:.6}, R {:.6}",
            fit.hr_ci_lower[i],
            rlo[i].as_f64().unwrap()
        );
        assert!(
            approx(fit.hr_ci_upper[i], rhi[i].as_f64().unwrap(), TOL_COEF),
            "Cox HR upper[{i}]: got {:.6}, R {:.6}",
            fit.hr_ci_upper[i],
            rhi[i].as_f64().unwrap()
        );
    }

    // ── p-values ───────────────────────────────────────────────────────
    let rp = ref_val["p_values"].as_array().unwrap();
    for i in 0..3 {
        let expected = rp[i].as_f64().unwrap();
        assert!(
            approx(fit.p_values[i], expected, 1e-3),
            "Cox p[{i}]: got {:.6e}, R {:.6e}",
            fit.p_values[i],
            expected
        );
    }

    // ── Log-likelihood ─────────────────────────────────────────────────
    let rll = ref_val["log_likelihood"].as_f64().unwrap();
    let rnll = ref_val["null_log_likelihood"].as_f64().unwrap();
    assert!(
        approx(fit.log_likelihood, rll, TOL_LL),
        "Cox LL: got {:.6}, R {:.6}",
        fit.log_likelihood,
        rll
    );
    assert!(
        approx(fit.null_log_likelihood, rnll, TOL_LL),
        "Cox null LL: got {:.6}, R {:.6}",
        fit.null_log_likelihood,
        rnll
    );

    // ── Concordance ────────────────────────────────────────────────────
    let rc_index = ref_val["concordance"].as_f64().unwrap();
    assert!(
        approx(fit.concordance, rc_index, TOL_C),
        "C-index: got {:.6}, R {:.6}",
        fit.concordance,
        rc_index
    );

    // ── n and events ───────────────────────────────────────────────────
    let rn = ref_val["n"].as_i64().unwrap() as usize;
    let rne = ref_val["n_events"].as_i64().unwrap() as usize;
    assert_eq!(fit.n_obs, rn, "n_obs mismatch");
    assert_eq!(fit.n_events, rne, "n_events mismatch");

    println!(
        "✅ Cox: 3 coefs, 3 SEs, 3 HRs, 3 HR CIs, 3 p-values, LL, null LL, C-index — all match R coxph"
    );
    println!("  coefs = {:?}", fit.coefficients);
    println!("  HRs   = {:?}", fit.hazard_ratios);
    println!("  C     = {:.4}", fit.concordance);
}
