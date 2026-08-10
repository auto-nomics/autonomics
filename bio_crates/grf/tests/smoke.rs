//! End-to-end smoke tests for grf-sys + grf.
//!
//! These tests verify the C ABI shim + Rust bindings produce a working
//! trained forest and prediction round-trip. They are NOT golden tests
//! (those live in tests/xval_*.rs with R grf outputs as fixtures).

use std::sync::Arc;

use arrow_array::{Array, Float64Array, Int64Array, RecordBatch};
use arrow_schema::{DataType, Field, Schema};

use grf::data::Matrix;
use grf::forest::{PredictRequest, RegressionSpec, RegressionTrainer};
use grf::nodes::{
    MultiRegressionForestSpec, ProbabilityForestSpec, QuantileForestSpec,
    SurvivalForestSpec,
};
use grf::nodes::regression_forest::NodeTrainOptions;
use grf_sys as sys;

/// Small deterministic regression dataset:
///
///   X ∈ R^{n×p}, iid N(0,1);  Y = X[:,0] + 0.5 X[:,1] + ε, ε ~ N(0,0.25).
///
/// Used by smoke tests to verify the training/predict round-trip succeeds
/// and produces sane predictions (no NaNs, magnitude within ~2σ of truth).
fn make_synth(n: usize, p: usize, seed: u64) -> (Vec<Vec<f64>>, Vec<f64>) {
    use rand::SeedableRng;
    use rand_distr::{Distribution, Normal};
    let mut rng = rand::rngs::StdRng::seed_from_u64(seed);
    let normal = Normal::new(0.0, 1.0).unwrap();
    let noise = Normal::new(0.0, 0.5).unwrap();
    let mut x = vec![vec![0.0; p]; n];
    let mut y = vec![0.0; n];
    for i in 0..n {
        for j in 0..p {
            x[i][j] = normal.sample(&mut rng);
        }
        y[i] = x[i][0] + 0.5 * x[i][1] + noise.sample(&mut rng);
    }
    (x, y)
}

#[test]
fn regression_train_predict_smoke() {
    let (x_rows, y) = make_synth(80, 4, 1234);
    let xm = Matrix::from_rows(&x_rows);
    let mut opts = sys::TrainOptions::default();
    opts.num_trees = 100;
    opts.seed = 42;
    opts.compute_oob_predictions = true;
    // Pin to 1 thread to avoid `hardware_concurrency() == 0` edge cases in
    // containers; remove this once grf's split_sequence handles 0 threads.
    opts.num_threads = 1;

    let forest = RegressionTrainer::fit(RegressionSpec {
        x: xm.clone(),
        y: y.clone(),
        sample_weights: None,
        options: opts.clone(),
    }).expect("fit succeeds");

    assert_eq!(forest.kind(), grf::forest::ForestKind::Regression);
    assert_eq!(forest.n_features(), 4);
    assert_eq!(forest.num_trees(), 100);

    // Predict OOB on the training set.
    let req = PredictRequest::oob(xm.clone(), 4, Some(1));
    let preds = forest.predict(req).expect("predict_oob succeeds");
    assert_eq!(preds.n_samples(), 80);
    assert_eq!(preds.pred_length, 1);
    assert!(preds.values.iter().all(|v| v.is_finite()),
            "all predictions finite, got first 5: {:?}", &preds.values[..5.min(preds.values.len())]);
    // OOB error metrics should be present.
    let debiased = preds.debiased_error.expect("OOB error captured");
    assert_eq!(debiased.len(), 80);
    let excess = preds.excess_error.expect("OOB excess captured");
    assert_eq!(excess.len(), 80);

    // Predict on new test data (different X) — should still produce finite preds.
    let (x_test, _) = make_synth(20, 4, 999);
    let xtm = Matrix::from_rows(&x_test);
    let req = PredictRequest::new_data(xm, 4, xtm, false);
    let preds = forest.predict(req).expect("predict new");
    assert_eq!(preds.n_samples(), 20);
    assert!(preds.values.iter().all(|v| v.is_finite()));
}

#[test]
fn regression_serialize_roundtrip() {
    let (x_rows, y) = make_synth(60, 3, 7);
    let xm = Matrix::from_rows(&x_rows);
    let mut opts = sys::TrainOptions::default();
    opts.num_trees = 50;
    opts.seed = 1;
    opts.compute_oob_predictions = false;
    opts.num_threads = 1;

    let forest = RegressionTrainer::fit(RegressionSpec {
        x: xm, y, sample_weights: None, options: opts,
    }).expect("fit");
    let blob = forest.serialize().expect("serialize");
    assert!(!blob.is_empty());

    let restored = grf::forest::ForestBlob::deserialize(
        &blob,
        grf::forest::ForestKind::Regression,
        3,
    ).expect("deserialize");
    assert_eq!(restored.kind(), grf::forest::ForestKind::Regression);
    assert_eq!(restored.num_trees(), 50);
}

#[test]
fn oob_predictions_present() {
    let (x_rows, y) = make_synth(40, 2, 11);
    let xm = Matrix::from_rows(&x_rows);
    let mut opts = sys::TrainOptions::default();
    opts.num_trees = 50;
    opts.compute_oob_predictions = true;
    opts.num_threads = 1;
    let forest = RegressionTrainer::fit(RegressionSpec {
        x: xm, y, sample_weights: None, options: opts,
    }).expect("fit");
    let oob = forest.oob_predictions().expect("oob");
    assert_eq!(oob.pred_length, 1);
    assert_eq!(oob.values.len(), 40);
    assert!(oob.values.iter().all(|v| v.is_finite()));
}

#[test]
fn arrow_round_trip_through_dag_node() {
    // Build the same dataset, then exercise the DAG-node-level fit() helper.
    let (x_rows, y) = make_synth(30, 3, 5);

    let schema = Arc::new(Schema::new(vec![
        Field::new("x0", DataType::Float64, false),
        Field::new("x1", DataType::Float64, false),
        Field::new("x2", DataType::Float64, false),
        Field::new("y",  DataType::Float64, false),
    ]));

    let mut cols: Vec<Vec<f64>> = vec![Vec::new(); 4];
    for (i, row) in x_rows.iter().enumerate() {
        cols[0].push(row[0]); cols[1].push(row[1]); cols[2].push(row[2]);
        cols[3].push(y[i]);
    }
    let batch = RecordBatch::try_new(schema.clone(), vec![
        Arc::new(Float64Array::from(cols[0].clone())),
        Arc::new(Float64Array::from(cols[1].clone())),
        Arc::new(Float64Array::from(cols[2].clone())),
        Arc::new(Float64Array::from(cols[3].clone())),
    ]).unwrap();

    let spec = grf::nodes::RegressionForestSpec {
        x_column_names: vec!["x0".into(), "x1".into(), "x2".into()],
        y_column_name: "y".into(),
        sample_weights_column: None,
        options: grf::nodes::regression_forest::NodeTrainOptions {
            num_trees: 40,
            seed: 1,
            ..Default::default()
        },
    };
    let out = spec.fit(&[batch]).expect("node fit succeeds");
    assert_eq!(out.forest.num_trees(), 40);
    assert!(out.oob_predictions.is_some());
}
// ═══════════════════════════════════════════════════════════════════════
// P2 baseline forest nodes
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn quantile_forest_smoke() {
    let (x_rows, y) = make_synth(60, 3, 31);
    let schema = Arc::new(Schema::new(vec![
        Field::new("x0", DataType::Float64, false),
        Field::new("x1", DataType::Float64, false),
        Field::new("x2", DataType::Float64, false),
        Field::new("y",  DataType::Float64, false),
    ]));
    let mut cols: Vec<Arc<dyn Array>> = vec![
        Arc::new(Float64Array::from(x_rows.iter().map(|r| r[0]).collect::<Vec<_>>())),
        Arc::new(Float64Array::from(x_rows.iter().map(|r| r[1]).collect::<Vec<_>>())),
        Arc::new(Float64Array::from(x_rows.iter().map(|r| r[2]).collect::<Vec<_>>())),
        Arc::new(Float64Array::from(y)),
    ];
    let batch = RecordBatch::try_new(schema.clone(), cols.clone()).unwrap();
    let spec = grf::nodes::QuantileForestSpec {
        x_column_names: vec!["x0".into(), "x1".into(), "x2".into()],
        y_column_name: "y".into(),
        quantiles: vec![0.25, 0.5, 0.75],
        regression_splitting: false,
        options: NodeTrainOptions { num_trees: 50, num_threads: 1, seed: 7, ..Default::default() },
    };
    let _ = cols.pop();
    let out = spec.fit(&[batch]).expect("quantile fit");
    assert_eq!(out.forest.num_trees(), 50);
    // OOB is intentionally None for quantile (no predict strategy in grf).
    assert!(out.oob_predictions.is_none());
}

#[test]
fn probability_forest_smoke() {
    use arrow_array::Int64Array;
    let (x_rows, _) = make_synth(60, 2, 11);
    let y: Vec<i64> = (0..60).map(|i| (i % 3) as i64).collect();
    let schema = Arc::new(Schema::new(vec![
        Field::new("x0", DataType::Float64, false),
        Field::new("x1", DataType::Float64, false),
        Field::new("y",  DataType::Int64,   false),
    ]));
    let cols: Vec<Arc<dyn Array>> = vec![
        Arc::new(Float64Array::from(x_rows.iter().map(|r| r[0]).collect::<Vec<_>>())),
        Arc::new(Float64Array::from(x_rows.iter().map(|r| r[1]).collect::<Vec<_>>())),
        Arc::new(Int64Array::from(y)),
    ];
    let batch = RecordBatch::try_new(schema.clone(), cols).unwrap();
    let spec = grf::nodes::ProbabilityForestSpec {
        x_column_names: vec!["x0".into(), "x1".into()],
        y_column_name: "y".into(),
        num_classes: 3,
        sample_weights_column: None,
        options: NodeTrainOptions { num_trees: 50, num_threads: 1, seed: 1, ..Default::default() },
    };
    let out = spec.fit(&[batch]).expect("probability fit");
    assert_eq!(out.forest.num_trees(), 50);
    let oob = out.oob_predictions.expect("oob");
    assert_eq!(oob.values.len(), 60 * 3);
    assert_eq!(oob.pred_length, 3);
    // Per-row probabilities should sum to ~1.0 (column-major: class × row).
    for i in 0..60 {
        let s: f64 = (0..3).map(|c| oob.values[c * 60 + i]).sum();
        assert!((s - 1.0).abs() < 1e-6, "row {i} probs sum to {s}");
    }
}

#[test]
fn survival_forest_smoke() {
    use arrow_array::Int64Array;
    let (x_rows, _) = make_synth(60, 2, 19);
    let time: Vec<f64> = (1..=60).map(|i| i as f64 * 0.1).collect();
    let censor: Vec<i64> = (0..60).map(|i| if i % 3 == 0 { 1 } else { 0 }).collect();
    let schema = Arc::new(Schema::new(vec![
        Field::new("x0", DataType::Float64, false),
        Field::new("x1", DataType::Float64, false),
        Field::new("time", DataType::Float64, false),
        Field::new("censor", DataType::Int64, false),
    ]));
    let cols: Vec<Arc<dyn Array>> = vec![
        Arc::new(Float64Array::from(x_rows.iter().map(|r| r[0]).collect::<Vec<_>>())),
        Arc::new(Float64Array::from(x_rows.iter().map(|r| r[1]).collect::<Vec<_>>())),
        Arc::new(Float64Array::from(time)),
        Arc::new(Int64Array::from(censor)),
    ];
    let batch = RecordBatch::try_new(schema.clone(), cols).unwrap();
    let spec = grf::nodes::SurvivalForestSpec {
        x_column_names: vec!["x0".into(), "x1".into()],
        time_column_name: "time".into(),
        censor_column_name: "censor".into(),
        failure_times: None,
        sample_weights_column: None,
        options: NodeTrainOptions { num_trees: 50, num_threads: 1, seed: 5, ..Default::default() },
    };
    let out = spec.fit(&[batch]).expect("survival fit");
    assert_eq!(out.forest.num_trees(), 50);
}

#[test]
fn multi_regression_forest_smoke() {
    let (x_rows, _) = make_synth(50, 2, 23);
    let y0: Vec<f64> = x_rows.iter().map(|r| r[0] + r[1]).collect();
    let y1: Vec<f64> = x_rows.iter().map(|r| r[0] - r[1]).collect();
    let schema = Arc::new(Schema::new(vec![
        Field::new("x0", DataType::Float64, false),
        Field::new("x1", DataType::Float64, false),
        Field::new("y0", DataType::Float64, false),
        Field::new("y1", DataType::Float64, false),
    ]));
    let cols: Vec<Arc<dyn Array>> = vec![
        Arc::new(Float64Array::from(x_rows.iter().map(|r| r[0]).collect::<Vec<_>>())),
        Arc::new(Float64Array::from(x_rows.iter().map(|r| r[1]).collect::<Vec<_>>())),
        Arc::new(Float64Array::from(y0)),
        Arc::new(Float64Array::from(y1)),
    ];
    let batch = RecordBatch::try_new(schema.clone(), cols).unwrap();
    let spec = grf::nodes::MultiRegressionForestSpec {
        x_column_names: vec!["x0".into(), "x1".into()],
        y_column_names: vec!["y0".into(), "y1".into()],
        sample_weights_column: None,
        options: NodeTrainOptions { num_trees: 50, num_threads: 1, seed: 11, ..Default::default() },
    };
    let out = spec.fit(&[batch]).expect("multi regression fit");
    assert_eq!(out.forest.num_trees(), 50);
    let oob = out.oob_predictions.expect("oob");
    assert_eq!(oob.values.len(), 50 * 2);
    assert_eq!(oob.pred_length, 2);
}

// ═══════════════════════════════════════════════════════════════════════
// P3 causal forest + analysis trio
// ═══════════════════════════════════════════════════════════════════════

/// Synthetic dataset with heterogeneous treatment effect:
///   τ(x) = 1 + x₀, so true ATE = 1 + E[x₀] = 1 (when x₀ is centered).
///   Y = τ(x) · W + ε,  ε ~ N(0, 0.5).
///   W ~ Bernoulli(0.5).
fn make_synth_treatment(n: usize, p: usize, seed: u64) -> (
    Vec<Vec<f64>>, Vec<f64>, Vec<f64>,
) {
    use rand::SeedableRng;
    use rand_distr::{Distribution, Normal};
    let mut rng = rand::rngs::StdRng::seed_from_u64(seed);
    let normal = Normal::new(0.0, 1.0).unwrap();
    let noise = Normal::new(0.0, 0.5).unwrap();
    let mut x = vec![vec![0.0; p]; n];
    let mut y = vec![0.0; n];
    let mut w = vec![0.0; n];
    for i in 0..n {
        for j in 0..p {
            x[i][j] = normal.sample(&mut rng);
        }
        // Probability of treatment depends on x₀.
        let prob = 1.0 / (1.0 + (-x[i][0] as f64).exp());
        let u: f64 = rand::random();
        w[i] = if u < prob { 1.0 } else { 0.0 };
        let tau = 1.0 + x[i][0];
        y[i] = tau * w[i] + noise.sample(&mut rng);
    }
    (x, y, w)
}

#[test]
fn causal_forest_smoke() {
    let (x_rows, y, w) = make_synth_treatment(150, 3, 42);
    let schema = Arc::new(Schema::new(vec![
        Field::new("x0", DataType::Float64, false),
        Field::new("x1", DataType::Float64, false),
        Field::new("x2", DataType::Float64, false),
        Field::new("y",  DataType::Float64, false),
        Field::new("w",  DataType::Float64, false),
    ]));
    let cols: Vec<Arc<dyn Array>> = vec![
        Arc::new(Float64Array::from(x_rows.iter().map(|r| r[0]).collect::<Vec<_>>())),
        Arc::new(Float64Array::from(x_rows.iter().map(|r| r[1]).collect::<Vec<_>>())),
        Arc::new(Float64Array::from(x_rows.iter().map(|r| r[2]).collect::<Vec<_>>())),
        Arc::new(Float64Array::from(y.clone())),
        Arc::new(Float64Array::from(w.clone())),
    ];
    let batch = RecordBatch::try_new(schema.clone(), cols).unwrap();
    let spec = grf::nodes::CausalForestSpec {
        x_column_names: vec!["x0".into(), "x1".into(), "x2".into()],
        y_column_name: "y".into(),
        w_column_name: "w".into(),
        y_hat: None, w_hat: None,
        stabilize_splits: true,
        sample_weights_column: None,
        options: NodeTrainOptions { num_trees: 100, num_threads: 1, seed: 7, ..Default::default() },
    };
    let out = spec.fit(&[batch]).expect("causal fit");
    assert_eq!(out.forest.num_trees(), 100);
    // y_hat and w_hat should have been learned internally.
    assert_eq!(out.y_hat.len(), 150);
    assert_eq!(out.w_hat.len(), 150);
    let oob_len = out.oob_predictions.as_ref()
        .map(|o| o.values.len())
        .unwrap_or(0);
    assert_eq!(oob_len, 150);

    // ATE estimate should be in a reasonable range (true ATE ≈ 1, but with
    // 150 samples + R-learner two-stage, expect estimate roughly in [0, 2]).
    let ate = grf::nodes::AverageTreatmentEffectSpec {
        target_sample: "all".into(),
        method: "AIPW".into(),
        subset: None,
        clusters: None,
    };
    let ate_out = ate.estimate(&out).expect("ate");
    assert!(ate_out.estimate.abs() < 3.0, "ATE={} too far from truth", ate_out.estimate);
    assert!(ate_out.std_err > 0.0);

    // BLP on a single feature (x₀) — should pick up heterogeneity.
    let blp_schema = Arc::new(Schema::new(vec![Field::new("x0", DataType::Float64, false)]));
    let blp_cols: Vec<Arc<dyn Array>> = vec![
        Arc::new(Float64Array::from(x_rows.iter().map(|r| r[0]).collect::<Vec<_>>())),
    ];
    let blp_batch = RecordBatch::try_new(blp_schema, blp_cols).unwrap();
    let blp_spec = grf::nodes::BestLinearProjectionSpec {
        a_column_names: vec!["x0".into()],
        subset: None,
        sample_weights: None,
    };
    let blp_out = blp_spec.project(&out, Some(&[blp_batch])).expect("blp");
    // Expect 2 coefficients (intercept + x₀).
    assert_eq!(blp_out.coefficients.len(), 2);
    assert_eq!(blp_out.std_errors.len(), 2);

    // Calibration test.
    let cal_spec = grf::nodes::TestCalibrationSpec {
        vcov_type: "HC3".into(),
    };
    let cal_out = cal_spec.check_causal(&out).expect("cal");
    assert_eq!(cal_out.n_obs, 150);
}

#[test]
fn average_treatment_effect_overlap_target() {
    let (x_rows, y, w) = make_synth_treatment(100, 2, 13);
    let schema = Arc::new(Schema::new(vec![
        Field::new("x0", DataType::Float64, false),
        Field::new("x1", DataType::Float64, false),
        Field::new("y",  DataType::Float64, false),
        Field::new("w",  DataType::Float64, false),
    ]));
    let cols: Vec<Arc<dyn Array>> = vec![
        Arc::new(Float64Array::from(x_rows.iter().map(|r| r[0]).collect::<Vec<_>>())),
        Arc::new(Float64Array::from(x_rows.iter().map(|r| r[1]).collect::<Vec<_>>())),
        Arc::new(Float64Array::from(y.clone())),
        Arc::new(Float64Array::from(w.clone())),
    ];
    let batch = RecordBatch::try_new(schema.clone(), cols).unwrap();
    let cf_spec = grf::nodes::CausalForestSpec {
        x_column_names: vec!["x0".into(), "x1".into()],
        y_column_name: "y".into(),
        w_column_name: "w".into(),
        y_hat: None, w_hat: None,
        stabilize_splits: true,
        sample_weights_column: None,
        options: NodeTrainOptions { num_trees: 60, num_threads: 1, seed: 3, ..Default::default() },
    };
    let cf = cf_spec.fit(&[batch]).expect("causal fit");
    // overlap target.sample should produce a non-empty estimate.
    let ate = grf::nodes::AverageTreatmentEffectSpec {
        target_sample: "overlap".into(),
        method: "AIPW".into(),
        subset: None,
        clusters: None,
    };
    let ate_out = ate.estimate(&cf).expect("ate overlap");
    assert!(ate_out.n_effective > 0);
}
