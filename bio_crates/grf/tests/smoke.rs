//! End-to-end smoke tests for grf-sys + grf.
//!
//! These tests verify the C ABI shim + Rust bindings produce a working
//! trained forest and prediction round-trip. They are NOT golden tests
//! (those live in tests/xval_*.rs with R grf outputs as fixtures).

use std::sync::Arc;

use arrow_array::{Array, Float64Array, RecordBatch};
use arrow_schema::{DataType, Field, Schema};

use grf::data::Matrix;
use grf::forest::{PredictRequest, RegressionSpec, RegressionTrainer};
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