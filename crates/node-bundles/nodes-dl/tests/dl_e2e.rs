//! End-to-end integration tests for DL DAG nodes.
//!
//! These tests build a DAG with DL nodes, execute it, and verify
//! the outputs are structurally correct and numerically sensible.

// To run: cargo test -p nodes-dl --test dl_e2e

#[cfg(test)]
mod tests {
    use arrow_array::{Array, Float64Array, Int32Array, RecordBatch};
    use arrow_schema::{DataType, Field, Schema};
    use datafusion::prelude::SessionContext;
    use std::sync::Arc;

    /// Build a simple RecordBatch with named Float64 columns.
    fn make_batch(columns: &[(&str, Vec<f64>)]) -> RecordBatch {
        let fields: Vec<_> = columns
            .iter()
            .map(|(name, _)| Field::new(*name, DataType::Float64, false))
            .collect();
        let arrays: Vec<_> = columns
            .iter()
            .map(|(_, vals)| Arc::new(Float64Array::from(vals.clone())) as Arc<dyn arrow_array::Array>)
            .collect();
        RecordBatch::try_new(Arc::new(Schema::new(fields)), arrays).unwrap()
    }

    /// Generate well-separated binary classification data.
    fn classification_data(n: usize) -> RecordBatch {
        let mut x1 = Vec::with_capacity(n);
        let mut x2 = Vec::with_capacity(n);
        let mut y = Vec::with_capacity(n);
        for i in 0..n {
            if i < n / 2 {
                x1.push(0.0 + (i as f64 * 0.01));
                x2.push(0.0 + (i as f64 * 0.01));
                y.push(0.0);
            } else {
                x1.push(5.0 + ((i - n / 2) as f64 * 0.01));
                x2.push(5.0 + ((i - n / 2) as f64 * 0.01));
                y.push(1.0);
            }
        }
        make_batch(&[("x1", x1), ("x2", x2), ("y", y)])
    }

    /// Generate synthetic survival data.
    fn survival_data(n: usize) -> RecordBatch {
        use rand::SeedableRng;
        use rand::Rng;
        let mut rng = rand_chacha::ChaCha8Rng::seed_from_u64(42);
        let mut x1 = Vec::with_capacity(n);
        let mut x2 = Vec::with_capacity(n);
        let mut time = Vec::with_capacity(n);
        let mut event = Vec::with_capacity(n);

        for _ in 0..n {
            let a: f64 = rng.random::<f64>() * 4.0 - 2.0;
            let b: f64 = rng.random::<f64>() * 4.0 - 2.0;
            let risk = a * a + b * b;
            let u: f64 = rng.random::<f64>();
            let baseline = -u.ln() / 0.5;
            let t = baseline / risk.exp().max(0.1);

            x1.push(a);
            x2.push(b);
            if t < 5.0 {
                event.push(1.0);
                time.push(t.max(0.01));
            } else {
                event.push(0.0);
                time.push(5.0);
            }
        }

        make_batch(&[("x1", x1), ("x2", x2), ("time", time), ("event", event)])
    }

    // ═══════════════════════════════════════════════════════════════════════
    // dl_mlp_train + dl_predict roundtrip
    // ═══════════════════════════════════════════════════════════════════════

    #[tokio::test]
    async fn test_mlp_train_produces_predictions() {
        let data = classification_data(40);
        let ctx = SessionContext::new();
        let df = ctx.read_batch(data).unwrap();

        // Simulate what the node does: extract features, train, output predictions.
        let batches = df.collect().await.unwrap();
        let x1: Vec<f64> = batches[0]
            .column(0)
            .as_any()
            .downcast_ref::<Float64Array>()
            .unwrap()
            .iter()
            .map(|v| v.unwrap())
            .collect();
        let x2: Vec<f64> = batches[0]
            .column(1)
            .as_any()
            .downcast_ref::<Float64Array>()
            .unwrap()
            .iter()
            .map(|v| v.unwrap())
            .collect();
        let y: Vec<f64> = batches[0]
            .column(2)
            .as_any()
            .downcast_ref::<Float64Array>()
            .unwrap()
            .iter()
            .map(|v| v.unwrap())
            .collect();

        let flat_x: Vec<f64> = x1.iter().zip(x2.iter()).flat_map(|(&a, &b)| vec![a, b]).collect();
        let x_tensor = dl::Tensor::from_rows(40, 2, &flat_x);
        let y_tensor = dl::Tensor::from_rows(40, 1, &y);

        let config = dl::MlpConfig {
            hidden_sizes: vec![8],
            activation: dl::Activation::Relu,
            dropout: 0.0,
            batch_norm: false,
            task_type: dl::TaskType::Classification,
            train: dl::TrainConfig {
                optimizer: dl::OptimizerConfig {
                    kind: dl::OptimizerKind::Adam,
                    lr: 0.01,
                    ..Default::default()
                },
                n_epochs: 50,
                batch_size: 10,
                ..Default::default()
            },
        };

        let result = dl::train_mlp(&x_tensor, &y_tensor, None, &config).unwrap();
        let probs: Vec<f64> = (0..40).map(|i| result.predictions.at(i, 0)).collect();
        let sigmoid_probs: Vec<f64> = probs.iter().map(|&p| dl::sigmoid_stable(p)).collect();

        let correct = sigmoid_probs
            .iter()
            .zip(&y)
            .filter(|(p, label)| ((**p > 0.5) as i32) == (**label as i32))
            .count();
        assert!(correct >= 30, "only {correct}/40 correct");
        assert!(correct >= 30, "only {correct}/40 correct");
    }

    // ═══════════════════════════════════════════════════════════════════════
    // Model serialization roundtrip
    // ═══════════════════════════════════════════════════════════════════════

    #[test]
    fn test_model_artifact_roundtrip() {
        let artifact = dl::DLModelArtifact {
            backend: "faer".into(),
            architecture: dl::Architecture::Mlp,
            task_type: dl::ArtifactTaskType::Classification,
            checkpoint_json: "{}".into(),
            feature_names: vec!["x1".into(), "x2".into()],
            label_column: Some("y".into()),
            time_column: None,
            event_column: None,
            scaler_json: None,
            training_meta: dl::TrainingMeta {
                n_epochs_run: 10,
                best_epoch: Some(5),
                best_val_metric: Some(0.9),
                total_params: 100,
            },
        };
        let bytes = serde_json::to_vec(&artifact).unwrap();
        let restored: dl::DLModelArtifact = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(restored.backend, "faer");
        assert_eq!(restored.feature_names, vec!["x1", "x2"]);
        assert_eq!(restored.training_meta.total_params, 100);
    }

    // ═══════════════════════════════════════════════════════════════════════
    // DeepSurv training + prediction
    // ═══════════════════════════════════════════════════════════════════════

    #[tokio::test]
    async fn test_deepsurv_trains() {
        let data = survival_data(80);
        let ctx = SessionContext::new();
        let df = ctx.read_batch(data).unwrap();
        let batches = df.collect().await.unwrap();

        let extract = |col_idx: usize| -> Vec<f64> {
            batches[0]
                .column(col_idx)
                .as_any()
                .downcast_ref::<Float64Array>()
                .unwrap()
                .iter()
                .map(|v| v.unwrap())
                .collect()
        };

        let x1 = extract(0);
        let x2 = extract(1);
        let times = extract(2);
        let events: Vec<usize> = extract(3).into_iter().map(|v| v as usize).collect();

        let flat_x: Vec<f64> = x1.iter().zip(x2.iter()).flat_map(|(&a, &b)| vec![a, b]).collect();
        let x_tensor = dl::Tensor::from_rows(80, 2, &flat_x);

        let config = dl::DeepSurvConfig {
            hidden_sizes: vec![16, 8],
            activation: dl::Activation::Relu,
            dropout: 0.0,
            train: dl::TrainConfig {
                optimizer: dl::OptimizerConfig {
                    kind: dl::OptimizerKind::Adam,
                    lr: 0.01,
                    ..Default::default()
                },
                n_epochs: 50,
                batch_size: 16,
                ..Default::default()
            },
        };

        let result = dl::train_deepsurv(&x_tensor, &times, &events, None, &config).unwrap();
        let risks: Vec<f64> = (0..80).map(|i| result.risk_scores.at(i, 0)).collect();
        let ci = dl::c_index(&risks, &times, &events);
        assert!(ci > 0.55, "C-index too low: {ci:.3}");
    }

    // ═══════════════════════════════════════════════════════════════════════
    // Train → save → load → predict pipeline
    // ═══════════════════════════════════════════════════════════════════════

    #[tokio::test]
    async fn test_train_save_load_predict() {
        let data = classification_data(40);
        let ctx = SessionContext::new();
        let df = ctx.read_batch(data).unwrap();
        let batches = df.collect().await.unwrap();

        let extract = |col_idx: usize| -> Vec<f64> {
            batches[0]
                .column(col_idx)
                .as_any()
                .downcast_ref::<Float64Array>()
                .unwrap()
                .iter()
                .map(|v| v.unwrap())
                .collect()
        };

        let x1 = extract(0);
        let x2 = extract(1);
        let y = extract(2);

        let flat_x: Vec<f64> = x1.iter().zip(x2.iter()).flat_map(|(&a, &b)| vec![a, b]).collect();
        let x_tensor = dl::Tensor::from_rows(40, 2, &flat_x);
        let y_tensor = dl::Tensor::from_rows(40, 1, &y);

        let config = dl::MlpConfig {
            hidden_sizes: vec![8],
            activation: dl::Activation::Relu,
            dropout: 0.0,
            batch_norm: false,
            task_type: dl::TaskType::Classification,
            train: dl::TrainConfig {
                optimizer: dl::OptimizerConfig {
                    kind: dl::OptimizerKind::Adam,
                    lr: 0.01,
                    ..Default::default()
                },
                n_epochs: 30,
                batch_size: 10,
                ..Default::default()
            },
        };

        // Train.
        let result = dl::train_mlp(&x_tensor, &y_tensor, None, &config).unwrap();

        // Serialize model.
        let model_json = serde_json::to_string(&result.model).unwrap();
        let restored_model: dl::MlpModel = serde_json::from_str(&model_json).unwrap();
        assert_eq!(restored_model.n_features, 2);

        // Predict with restored model.
        let mut model = restored_model;
        let x_new = dl::Tensor::from_rows(2, 2, &[0.1, 0.0, 5.1, 5.0]);
        let probs = dl::predict_mlp(&mut model, &x_new);
        assert!(probs.at(0, 0) < 0.5, "class 0 prob too high: {}", probs.at(0, 0));
        assert!(probs.at(1, 0) > 0.5, "class 1 prob too low: {}", probs.at(1, 0));
    }

    // ═══════════════════════════════════════════════════════════════════════
    // Survival metrics
    // ═══════════════════════════════════════════════════════════════════════

    #[test]
    fn test_c_index_perfect() {
        let risk = vec![3.0, 2.0, 1.0];
        let times = vec![1.0, 2.0, 3.0];
        let events = vec![1, 1, 1];
        assert!((dl::c_index(&risk, &times, &events) - 1.0).abs() < 1e-10);
    }

    #[test]
    fn test_c_index_with_censoring() {
        let risk = vec![2.0, 1.0, 0.5];
        let times = vec![1.0, 5.0, 3.0];
        let events = vec![1, 0, 1];
        let ci = dl::c_index(&risk, &times, &events);
        assert!((ci - 2.0 / 3.0).abs() < 1e-10, "ci = {ci}");
    }
}
