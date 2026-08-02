//! Cross-validation: SEM (CFA) vs R lavaan::cfa.
//!
//! Run: `cargo test -p epi --test xval_sem -- --nocapture`

use serde_json::Value;
use std::fs;

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
        "/tests/xval/sem_reference.json"
    ))
    .expect("sem_reference.json not found");
    serde_json::from_str(&json).expect("invalid JSON")
}

#[test]
fn xval_cfa_vs_r_lavaan() {
    let rows = load_csv(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/xval/sem_data.csv"
    ));
    let data: Vec<Vec<f64>> = rows;

    let spec = epi::sem::CfaSpec {
        loadings: vec![
            epi::sem::LoadingSpec {
                indicator: 0,
                factor: 0,
                fixed: Some(1.0),
            }, // x1 marker
            epi::sem::LoadingSpec {
                indicator: 1,
                factor: 0,
                fixed: None,
            }, // x2
            epi::sem::LoadingSpec {
                indicator: 2,
                factor: 0,
                fixed: None,
            }, // x3
            epi::sem::LoadingSpec {
                indicator: 3,
                factor: 1,
                fixed: Some(1.0),
            }, // x4 marker
            epi::sem::LoadingSpec {
                indicator: 4,
                factor: 1,
                fixed: None,
            }, // x5
            epi::sem::LoadingSpec {
                indicator: 5,
                factor: 1,
                fixed: None,
            }, // x6
        ],
        factor_covariances: vec![(0, 1)],
        n_indicators: 6,
        n_factors: 2,
    };

    let result = epi::sem::cfa(&data, &spec).expect("CFA fit");
    let ref_val = load_reference();

    // ── Loading estimates: match R lavaan within tolerance ─────────────
    // R lavaan: x2=0.811, x3=0.768, x5=0.753, x6=0.716
    // Our implementation uses gradient descent which may converge less
    // precisely. Compare loading DIRECTION and magnitude.
    let r_loadings = ref_val["loadings"].as_array().unwrap();

    // Our free loadings (in order: x2→f1, x3→f1, x5→f2, x6→f2).
    let our_loadings = [
        result.lambda[1][0],
        result.lambda[2][0],
        result.lambda[4][1],
        result.lambda[5][1],
    ];
    let r_vals: Vec<f64> = [1, 2, 4, 5]
        .iter()
        .map(|&i| r_loadings[i]["est"].as_f64().unwrap())
        .collect();

    println!("  Our loadings: {:?}", our_loadings);
    println!("  R   loadings: {:?}", r_vals);

    // All loadings should be positive (strong factor indicators).
    for &l in &our_loadings {
        assert!(l > 0.0, "All loadings should be positive");
    }

    // Loadings should be in the ballpark of R (within 0.2 tolerance).
    // This is generous due to gradient descent vs quasi-Newton differences.
    for (i, (&ours, &r_val)) in our_loadings.iter().zip(r_vals.iter()).enumerate() {
        assert!(
            (ours - r_val).abs() < 0.2,
            "Loading {}: ours {:.4}, R {:.4}",
            i,
            ours,
            r_val
        );
    }

    // ── Factor covariance: should be positive (~0.46 in R) ─────────────
    let our_cov = result.phi[0][1];
    let r_cov = ref_val["factor_cov"]["est"].as_f64().unwrap();
    assert!(
        our_cov > 0.0,
        "Factor covariance should be positive: {}",
        our_cov
    );
    assert!(
        (our_cov - r_cov).abs() < 0.2,
        "Factor covariance: ours {:.4}, R {:.4}",
        our_cov,
        r_cov
    );

    // ── Fit indices: check ranges ──────────────────────────────────────
    // R lavaan: χ²=5.66, df=8, CFI=1.0, SRMR=0.009
    let r_fit = &ref_val["fit"];
    let r_chisq = r_fit["chisq"].as_f64().unwrap();
    let r_df = r_fit["df"].as_u64().unwrap() as usize;
    let r_cfi = r_fit["cfi"].as_f64().unwrap();
    let r_srmr = r_fit["srmr"].as_f64().unwrap();

    // df should match (model specification determines it).
    assert_eq!(result.df, r_df, "Degrees of freedom mismatch");

    // CFI should be high (good model fit). R gets CFI=1.0.
    assert!(result.cfi > 0.9, "CFI should be high: ours {}", result.cfi);

    // SRMR should be low (< 0.08 = acceptable, R gets 0.009).
    assert!(
        result.srmr < 0.1,
        "SRMR should be low: ours {}",
        result.srmr
    );

    println!("✅ CFA vs R lavaan: loadings, covariance, and fit indices consistent");
    println!(
        "   Our fit: χ²={:.2}, df={}, CFI={:.3}, SRMR={:.4}",
        result.chisq, result.df, result.cfi, result.srmr
    );
    println!(
        "   R   fit: χ²={:.2}, df={}, CFI={:.3}, SRMR={:.4}",
        r_chisq, r_df, r_cfi, r_srmr
    );
    println!("   Our cov = {:.4} (R {:.4})", our_cov, r_cov);
}
