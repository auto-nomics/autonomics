//! End-to-end integration tests for DL DAG nodes (Burn backend).

#[cfg(test)]
mod tests {
    use dl::*;
    use rand::Rng;
    use rand::SeedableRng;
    use rand_chacha::ChaCha8Rng;

    fn make_classification_data(n: usize, seed: u64) -> (Tensor, Tensor) {
        let mut rng = ChaCha8Rng::seed_from_u64(seed);
        let mut x = Vec::with_capacity(n * 4);
        let mut y = Vec::with_capacity(n);
        for _ in 0..n {
            let cls = if rng.random::<f64>() < 0.5 { 0 } else { 1 };
            let center = if cls == 0 { -1.0f64 } else { 1.0 };
            for _ in 0..4 {
                x.push(center + rng.random::<f64>() * 0.5);
            }
            y.push(cls as f64);
        }
        (Tensor::from_rows(n, 4, &x), Tensor::from_rows(n, 1, &y))
    }

    fn make_survival_data(n: usize, seed: u64) -> (Tensor, Vec<f64>, Vec<usize>) {
        let mut rng = ChaCha8Rng::seed_from_u64(seed);
        let mut x = Vec::with_capacity(n * 4);
        let mut times = Vec::with_capacity(n);
        let mut events = Vec::with_capacity(n);
        for _ in 0..n {
            let x1 = rng.random::<f64>() * 4.0 - 2.0;
            let x2 = rng.random::<f64>() * 4.0 - 2.0;
            let x3 = rng.random::<f64>() * 2.0;
            let x4 = rng.random::<f64>() * 2.0;
            let risk = x1 * x1 + x2 * x2;
            let u = rng.random::<f64>();
            let t = -u.ln() / (0.3 * risk.exp().max(0.1));
            x.extend_from_slice(&[x1, x2, x3, x4]);
            if t < 5.0 {
                events.push(1);
                times.push(t.max(0.01));
            } else {
                events.push(0);
                times.push(5.0);
            }
        }
        (Tensor::from_rows(n, 4, &x), times, events)
    }

    #[test]
    fn test_mlp_train_produces_predictions() {
        let (x, y) = make_classification_data(40, 42);
        let config = MlpConfig {
            hidden_sizes: vec![16],
            activation: Activation::Relu,
            dropout: 0.0,
            batch_norm: false,
            task_type: TaskType::Classification,
            train: TrainConfig {
                optimizer: OptimizerConfig {
                    kind: OptimizerKind::Adam,
                    lr: 0.01,
                    ..Default::default()
                },
                n_epochs: 50,
                batch_size: 10,
                ..Default::default()
            },
        };
        let result = train_mlp(&x, &y, None, &config).unwrap();
        assert_eq!(result.predictions.nrows(), 40);
    }

    #[test]
    fn test_model_artifact_roundtrip() {
        let artifact = DLModelArtifact {
            backend: "burn".into(),
            architecture: Architecture::Mlp,
            task_type: ArtifactTaskType::Classification,
            checkpoint_json: "{}".into(),
            feature_names: vec!["x1".into(), "x2".into()],
            label_column: Some("y".into()),
            time_column: None,
            event_column: None,
            scaler_json: None,
            training_meta: TrainingMeta {
                n_epochs_run: 10,
                best_epoch: Some(5),
                best_val_metric: Some(0.9),
                total_params: 100,
            },
        };
        let bytes = artifact.to_bytes().unwrap();
        let restored: DLModelArtifact = DLModelArtifact::from_bytes(&bytes).unwrap();
        assert_eq!(restored.backend, "burn");
        assert_eq!(restored.feature_names, vec!["x1", "x2"]);
    }

    #[test]
    fn test_deepsurv_trains() {
        let (x, times, events) = make_survival_data(80, 42);
        let config = DeepSurvConfig {
            hidden_sizes: vec![16, 8],
            activation: Activation::Relu,
            dropout: 0.0,
            train: TrainConfig {
                optimizer: OptimizerConfig {
                    kind: OptimizerKind::Adam,
                    lr: 0.01,
                    ..Default::default()
                },
                n_epochs: 50,
                batch_size: 16,
                ..Default::default()
            },
        };
        let result = train_deepsurv(&x, &times, &events, None, &config).unwrap();
        let risks: Vec<f64> = (0..80).map(|i| result.risk_scores.at(i, 0)).collect();
        let ci = c_index(&risks, &times, &events);
        assert!(ci > 0.55, "C-index too low: {ci:.3}");
    }

    #[test]
    fn test_train_save_load_predict() {
        let (x, y) = make_classification_data(40, 42);
        let config = MlpConfig {
            hidden_sizes: vec![8],
            activation: Activation::Relu,
            dropout: 0.0,
            batch_norm: false,
            task_type: TaskType::Classification,
            train: TrainConfig {
                optimizer: OptimizerConfig {
                    kind: OptimizerKind::Adam,
                    lr: 0.01,
                    ..Default::default()
                },
                n_epochs: 30,
                batch_size: 10,
                ..Default::default()
            },
        };
        let result = train_mlp(&x, &y, None, &config).unwrap();
        let json = serde_json::to_string(&result.model).unwrap();
        let mut restored: MlpModel = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.n_features, 4);
        let x_new = Tensor::from_rows(2, 4, &[-1.0, -1.0, -1.0, -1.0, 1.0, 1.0, 1.0, 1.0]);
        let probs = predict_mlp(&mut restored, &x_new);
        assert_eq!(probs.nrows(), 2);
    }

    #[test]
    fn test_c_index_perfect() {
        let risk = vec![3.0, 2.0, 1.0];
        let times = vec![1.0, 2.0, 3.0];
        let events = vec![1, 1, 1];
        assert!((c_index(&risk, &times, &events) - 1.0).abs() < 1e-10);
    }

    #[test]
    fn test_c_index_with_censoring() {
        let risk = vec![2.0, 1.0, 0.5];
        let times = vec![1.0, 5.0, 3.0];
        let events = vec![1, 0, 1];
        let ci = c_index(&risk, &times, &events);
        assert!((ci - 2.0 / 3.0).abs() < 1e-10, "ci = {ci}");
    }
}
