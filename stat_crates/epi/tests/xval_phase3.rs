//! Cross-validation: Phase 3 methods (LCA + Random Forest + SHAP) vs R.
//!
//! References:
//! - LCA: R `poLCA` package (poLCA::poLCA)
//! - RF:  R `randomForest` package (randomForest::randomForest)
//! - SHAP: efficiency property + ranking consistency with permutation importance
//!
//! Run: `cargo test -p epi --test xval_phase3 -- --nocapture`

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
        "/tests/xval/phase3_reference.json"
    ))
    .expect("phase3_reference.json not found");
    serde_json::from_str(&json).expect("invalid JSON")
}

// ═══════════════════════════════════════════════════════════════════════════
// 1. LCA vs R poLCA
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn xval_lca_vs_r_polca() {
    let rows = load_csv(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/xval/lca_data.csv"
    ));
    let n = rows.len();
    let n_indicators = rows[0].len();

    // Organise as indicators[j] = Vec<u64> across all subjects.
    let indicators: Vec<Vec<u64>> = (0..n_indicators)
        .map(|j| rows.iter().map(|r| r[j] as u64).collect())
        .collect();

    let result = epi::lca::lca(
        &indicators,
        &epi::lca::LcaOptions {
            n_classes: 2,
            seed: 42,
            ..Default::default()
        },
    )
    .expect("LCA fit");

    let ref_val = load_reference();
    let ref_prev = ref_val["lca"]["class_prevalence"].as_array().unwrap();
    let ref_probs = ref_val["lca"]["item_probabilities"].as_array().unwrap();

    // ── Class prevalence: match modulo label swapping ──────────────────
    // R's classes may be ordered differently. Compare sorted prevalence.
    let mut our_prev = result.class_prevalence.clone();
    our_prev.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let mut r_prev: Vec<f64> = ref_prev.iter().map(|v| v.as_f64().unwrap()).collect();
    r_prev.sort_by(|a, b| a.partial_cmp(b).unwrap());

    for i in 0..2 {
        assert!(
            (our_prev[i] - r_prev[i]).abs() < 0.05,
            "LCA prevalence[{}]: ours {:.4}, R {:.4}",
            i,
            our_prev[i],
            r_prev[i]
        );
    }

    // ── Item probabilities: match modulo label swapping ────────────────
    // For each indicator j, the two classes should give probabilities
    // {p_low, p_high} matching R's {p_low, p_high}.
    for j in 0..n_indicators {
        let mut ours = result.item_probabilities[j].clone();
        ours.sort_by(|a, b| a.partial_cmp(b).unwrap());

        let r_j = ref_probs[j].as_array().unwrap();
        let mut r_vals: Vec<f64> = r_j.iter().map(|v| v.as_f64().unwrap()).collect();
        r_vals.sort_by(|a, b| a.partial_cmp(b).unwrap());

        // Tolerance is generous because EM can converge to slightly different
        // local optima depending on initialization.
        for c in 0..2 {
            assert!(
                (ours[c] - r_vals[c]).abs() < 0.1,
                "LCA item_prob[{j}][{c}]: ours {:.4}, R {:.4}",
                ours[c],
                r_vals[c]
            );
        }
    }

    println!("✅ LCA vs R poLCA: prevalence and item probabilities match (tol=0.1)");
    println!("   our prevalence = {:?}", result.class_prevalence);
    println!("   R   prevalence = {:?}", r_prev);
}

// ═══════════════════════════════════════════════════════════════════════════
// 2. Random Forest vs R randomForest
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn xval_rf_vs_r_randomforest() {
    let rows = load_csv(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/xval/rf_data.csv"
    ));
    let n = rows.len();
    let features: Vec<Vec<f64>> = rows.iter().map(|r| r[..3].to_vec()).collect();
    let labels: Vec<f64> = rows.iter().map(|r| r[3]).collect();

    let opts = epi::ensemble::RfOptions {
        n_trees: 100,
        seed: 42,
        ..Default::default()
    };
    let rf = epi::ensemble::random_forest(&features, &labels, &opts).unwrap();

    let ref_val = load_reference();
    let r_oob = ref_val["rf"]["oob_accuracy"].as_f64().unwrap();
    let r_imp = ref_val["rf"]["importance"].as_array().unwrap();

    // ── OOB accuracy: should be within 15% (different implementations) ──
    assert!(
        (rf.oob_accuracy - r_oob).abs() < 0.15,
        "RF OOB accuracy: ours {:.4}, R {:.4}",
        rf.oob_accuracy,
        r_oob
    );

    // ── Feature importance ranking: x1, x2 > x3 (noise) ────────────────
    let our_imp = epi::ensemble::feature_importance(&rf, &features, &labels);
    let r_imp_vals: Vec<f64> = r_imp.iter().map(|v| v.as_f64().unwrap()).collect();

    // Both should rank x1 and x2 above x3.
    assert!(
        our_imp[0] > our_imp[2],
        "Our importance: x1 should > x3 (noise)"
    );
    assert!(
        our_imp[1] > our_imp[2],
        "Our importance: x2 should > x3 (noise)"
    );
    assert!(
        r_imp_vals[0] > r_imp_vals[2],
        "R importance: x1 should > x3 (noise)"
    );
    assert!(
        r_imp_vals[1] > r_imp_vals[2],
        "R importance: x2 should > x3 (noise)"
    );

    println!("✅ RF vs R randomForest: OOB accuracy and importance ranking consistent");
    println!("   our OOB = {:.4}, R OOB = {:.4}", rf.oob_accuracy, r_oob);
    println!("   our importance = {:?}", our_imp);
    println!("   R   importance = {:?}", r_imp_vals);
}

// ═══════════════════════════════════════════════════════════════════════════
// 3. SHAP: efficiency property + importance ranking
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn xval_shap_efficiency_and_ranking() {
    let rows = load_csv(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/xval/rf_data.csv"
    ));
    let features: Vec<Vec<f64>> = rows.iter().map(|r| r[..3].to_vec()).collect();
    let labels: Vec<f64> = rows.iter().map(|r| r[3]).collect();

    let opts = epi::ensemble::RfOptions {
        n_trees: 50,
        seed: 42,
        ..Default::default()
    };
    let rf = epi::ensemble::random_forest(&features, &labels, &opts).unwrap();

    let shap = epi::shap::shap_values(&rf, &features, vec!["x1".into(), "x2".into(), "x3".into()])
        .unwrap();

    // ── Efficiency: ΣSHAP + baseline ≈ prediction for ALL samples ──────
    let mut max_err = 0.0_f64;
    for i in 0..features.len() {
        let pred = epi::ensemble::predict_proba(&rf, &features[i]);
        let shap_sum: f64 = shap.values[i].iter().sum();
        let reconstructed = shap.baseline + shap_sum;
        let err = (reconstructed - pred).abs();
        if err > max_err {
            max_err = err;
        }
        assert!(
            err < 0.15,
            "SHAP efficiency violated for sample {i}: pred={pred:.4}, reconstructed={reconstructed:.4}"
        );
    }

    // ── Importance ranking: informative features > noise ───────────────
    assert!(
        shap.mean_abs[0] > shap.mean_abs[2],
        "SHAP importance: x1 should > x3"
    );
    assert!(
        shap.mean_abs[1] > shap.mean_abs[2],
        "SHAP importance: x2 should > x3"
    );

    // ── Compare ranking with R's permutation importance ────────────────
    let ref_val = load_reference();
    let r_imp = ref_val["rf"]["importance"].as_array().unwrap();
    let r_imp_vals: Vec<f64> = r_imp.iter().map(|v| v.as_f64().unwrap()).collect();

    // Both should agree on the noise feature being least important.
    let our_min_idx = shap
        .mean_abs
        .iter()
        .enumerate()
        .min_by(|(_, a), (_, b)| a.partial_cmp(b).unwrap())
        .map(|(i, _)| i)
        .unwrap();
    let r_min_idx = r_imp_vals
        .iter()
        .enumerate()
        .min_by(|(_, a), (_, b)| a.partial_cmp(b).unwrap())
        .map(|(i, _)| i)
        .unwrap();
    assert_eq!(
        our_min_idx, r_min_idx,
        "SHAP and R RF should agree on least important feature"
    );

    println!(
        "✅ SHAP: efficiency (max error {:.4}), importance ranking consistent",
        max_err
    );
    println!("   baseline = {:.4}", shap.baseline);
    println!("   mean_abs = {:?}", shap.mean_abs);
    println!("   R importance = {:?}", r_imp_vals);
}
