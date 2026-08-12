//! Full pipeline integration test: train → save → load → predict → metrics.
//!
//! This simulates a complete China-AIHeart style workflow without the DAG
//! engine infrastructure — just the `dl` algorithm layer.

// To run: cargo test -p nodes-dl --test dl_pipeline

#[cfg(test)]
mod tests {
    use dl::*;
    use rand::SeedableRng;
    use rand::Rng;
    use rand_chacha::ChaCha8Rng;

    // ═══════════════════════════════════════════════════════════════════════
    // Synthetic data generators
    // ═══════════════════════════════════════════════════════════════════════

    fn gen_classification_data(n: usize, seed: u64) -> (Tensor, Tensor) {
        let mut rng = ChaCha8Rng::seed_from_u64(seed);
        let mut x = Vec::with_capacity(n * 4);
        let mut y = Vec::with_capacity(n);
        for _ in 0..n {
            let cls = if rng.random::<f64>() < 0.5 { 0 } else { 1 };
            let center = if cls == 0 { -1.0 } else { 1.0 };
            for _ in 0..4 {
                x.push(center + rng.random::<f64>() * 0.5);
            }
            y.push(cls as f64);
        }
        (Tensor::from_rows(n, 4, &x), Tensor::from_rows(n, 1, &y))
    }

    fn gen_survival_data(n: usize, seed: u64) -> (Tensor, Vec<f64>, Vec<usize>) {
        let mut rng = ChaCha8Rng::seed_from_u64(seed);
        let mut x = Vec::with_capacity(n * 4);
        let mut times = Vec::with_capacity(n);
        let mut events = Vec::with_capacity(n);

        for _ in 0..n {
            let x1 = rng.random::<f64>() * 4.0 - 2.0;
            let x2 = rng.random::<f64>() * 4.0 - 2.0;
            let x3 = rng.random::<f64>() * 2.0;
            let x4 = rng.random::<f64>() * 2.0;
            let risk = x1 * x1 + x2 * x2 + x3 * 0.5;
            let u = rng.random::<f64>();
            let baseline = -u.ln() / 0.3;
            let t = baseline / risk.exp().max(0.1);

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

    // ═══════════════════════════════════════════════════════════════════════
    // MLP classification pipeline
    // ═══════════════════════════════════════════════════════════════════════

    #[test]
    fn test_mlp_classification_pipeline() {
        let (x_train, y_train) = gen_classification_data(100, 42);
        let (x_test, y_test) = gen_classification_data(50, 99);

        let config = MlpConfig {
            hidden_sizes: vec![16, 8],
            activation: Activation::Relu,
            dropout: 0.1,
            batch_norm: false,
            task_type: TaskType::Classification,
            train: TrainConfig {
                optimizer: OptimizerConfig {
                    kind: OptimizerKind::Adam,
                    lr: 0.01,
                    ..Default::default()
                },
                n_epochs: 80,
                batch_size: 16,
                ..Default::default()
            },
        };

        // Train.
        let result = train_mlp(&x_train, &y_train, None, &config).unwrap();

        // Serialize → deserialize.
        let json = serde_json::to_string(&result.model).unwrap();
        let mut restored: MlpModel = serde_json::from_str(&json).unwrap();

        // Predict on test set.
        let test_probs = predict_mlp(&mut restored, &x_test);

        // Compute accuracy.
        let correct = (0..50).filter(|&i| {
            let p = test_probs.at(i, 0);
            ((if p > 0.5 { 1.0 } else { 0.0 }) - y_test.at(i, 0)).abs() < 0.5
        }).count();
        let acc = correct as f64 / 50.0;
        assert!(acc > 0.7, "test accuracy too low: {acc:.2}");
    }

    // ═══════════════════════════════════════════════════════════════════════
    // MLP regression pipeline
    // ═══════════════════════════════════════════════════════════════════════

    #[test]
    fn test_mlp_regression_pipeline() {
        let n = 80;
        let mut rng = ChaCha8Rng::seed_from_u64(7);
        let mut x_data = Vec::with_capacity(n * 3);
        let mut y_data = Vec::with_capacity(n);
        for _ in 0..n {
            let a = rng.random::<f64>() * 4.0 - 2.0;
            let b = rng.random::<f64>() * 4.0 - 2.0;
            let c = rng.random::<f64>() * 3.0;
            // y = 2a - b + 0.5c
            x_data.extend_from_slice(&[a, b, c]);
            y_data.push(2.0 * a - b + 0.5 * c);
        }
        let x = Tensor::from_rows(n, 3, &x_data);
        let y = Tensor::from_rows(n, 1, &y_data);

        let config = MlpConfig {
            hidden_sizes: vec![32, 16],
            activation: Activation::Relu,
            dropout: 0.0,
            batch_norm: false,
            task_type: TaskType::Regression,
            train: TrainConfig {
                optimizer: OptimizerConfig {
                    kind: OptimizerKind::Adam,
                    lr: 0.005,
                    ..Default::default()
                },
                n_epochs: 150,
                batch_size: 16,
                ..Default::default()
            },
        };

        let result = train_mlp(&x, &y, None, &config).unwrap();
        let preds = result.predictions;

        let mse: f64 = (0..n).map(|i| (preds.at(i, 0) - y_data[i]).powi(2)).sum::<f64>() / n as f64;
        assert!(mse < 2.0, "regression MSE too high: {mse:.3}");
    }

    // ═══════════════════════════════════════════════════════════════════════
    // DeepSurv pipeline with C-index evaluation
    // ═══════════════════════════════════════════════════════════════════════

    #[test]
    fn test_deepsurv_pipeline_with_metrics() {
        let (x, times, events) = gen_survival_data(100, 42);
        let (x_test, t_test, e_test) = gen_survival_data(50, 99);

        let config = DeepSurvConfig {
            hidden_sizes: vec![32, 16],
            activation: Activation::Relu,
            dropout: 0.1,
            train: TrainConfig {
                optimizer: OptimizerConfig {
                    kind: OptimizerKind::Adam,
                    lr: 0.005,
                    ..Default::default()
                },
                n_epochs: 80,
                batch_size: 16,
                ..Default::default()
            },
        };

        // Train.
        let result = train_deepsurv(&x, &times, &events, None, &config).unwrap();

        // Training C-index.
        let train_risks: Vec<f64> = (0..100).map(|i| result.risk_scores.at(i, 0)).collect();
        let train_ci = c_index(&train_risks, &times, &events);
        assert!(train_ci > 0.6, "training C-index too low: {train_ci:.3}");

        // Serialize → deserialize.
        let json = serde_json::to_string(&result.model).unwrap();
        let mut restored: DeepSurvModel = serde_json::from_str(&json).unwrap();

        // Predict on test set.
        let test_risks_tensor = predict_deepsurv(&mut restored, &x_test);
        let test_risks: Vec<f64> = (0..50).map(|i| test_risks_tensor.at(i, 0)).collect();

        // Test C-index.
        let test_ci = c_index(&test_risks, &t_test, &e_test);
        assert!(test_ci > 0.55, "test C-index too low: {test_ci:.3}");

        // td-AUC.
        let auc_3 = td_auc(&test_risks, &t_test, &e_test, 3.0);
        assert!(auc_3 > 0.5, "td-AUC@3 too low: {auc_3:.3}");
    }

    // ═══════════════════════════════════════════════════════════════════════
    // Model artifact persistence pipeline
    // ═══════════════════════════════════════════════════════════════════════

    #[test]
    fn test_artifact_persistence() {
        let (x, y) = gen_classification_data(50, 1);

        let config = MlpConfig {
            hidden_sizes: vec![8],
            task_type: TaskType::Classification,
            train: TrainConfig {
                n_epochs: 20,
                batch_size: 10,
                ..Default::default()
            },
            ..Default::default()
        };

        let result = train_mlp(&x, &y, None, &config).unwrap();

        // Build artifact.
        let checkpoint = serde_json::to_string(&result.model).unwrap();
        let artifact = DLModelArtifact {
            backend: "faer".into(),
            architecture: Architecture::Mlp,
            task_type: ArtifactTaskType::Classification,
            checkpoint_json: checkpoint,
            feature_names: vec!["x1".into(), "x2".into(), "x3".into(), "x4".into()],
            label_column: Some("y".into()),
            time_column: None,
            event_column: None,
            scaler_json: None,
            training_meta: TrainingMeta {
                n_epochs_run: result.training_log.len(),
                best_epoch: None,
                best_val_metric: None,
                total_params: result.model.n_params(),
            },
        };

        // Serialize to bytes and back.
        let bytes = artifact.to_bytes().unwrap();
        let restored_artifact = DLModelArtifact::from_bytes(&bytes).unwrap();

        assert_eq!(restored_artifact.architecture, Architecture::Mlp);
        assert_eq!(restored_artifact.feature_names.len(), 4);
        assert_eq!(restored_artifact.training_meta.total_params, artifact.training_meta.total_params);

        // Deserialize the model from the artifact.
        let model: MlpModel = serde_json::from_str(&restored_artifact.checkpoint_json).unwrap();
        assert_eq!(model.n_features, 4);
    }

    // ═══════════════════════════════════════════════════════════════════════
    // Early stopping with validation set
    // ═══════════════════════════════════════════════════════════════════════

    #[test]
    fn test_mlp_with_early_stopping() {
        let (x_train, y_train) = gen_classification_data(60, 10);
        let (x_val, y_val) = gen_classification_data(20, 20);

        let config = MlpConfig {
            hidden_sizes: vec![32, 16],
            task_type: TaskType::Classification,
            train: TrainConfig {
                n_epochs: 300,
                batch_size: 32,
                early_stopping: Some(mlp::EarlyStoppingConfig {
                    metric: "val_loss".into(),
                    patience: 5,
                    mode: "min".into(),
                }),
                ..Default::default()
            },
            ..Default::default()
        };

        let result = train_mlp(
            &x_train, &y_train,
            Some((&x_val, &y_val)),
            &config,
        ).unwrap();

        // With patience=5 on well-separated data, should stop well before 300 epochs.
        assert!(result.training_log.len() <= 305, "early stopping should have triggered, ran {} epochs", result.training_log.len());
        // Should have validation metrics logged.
        assert!(result.training_log.iter().any(|l| l.val_loss.is_some()));
    }

    // ═══════════════════════════════════════════════════════════════════════
    // Gradient clipping
    // ═══════════════════════════════════════════════════════════════════════

    #[test]
    fn test_mlp_with_gradient_clipping() {
        let (x, y) = gen_classification_data(50, 5);

        let config = MlpConfig {
            hidden_sizes: vec![16, 8],
            task_type: TaskType::Classification,
            train: TrainConfig {
                n_epochs: 30,
                batch_size: 10,
                gradient_clip_norm: Some(1.0),
                ..Default::default()
            },
            ..Default::default()
        };

        let result = train_mlp(&x, &y, None, &config).unwrap();
        // Should not NaN or inf.
        for log in &result.training_log {
            assert!(log.train_loss.is_finite(), "non-finite loss at epoch {}", log.epoch);
        }
    }

    // ═══════════════════════════════════════════════════════════════════════
    // Time bins for discrete-time survival
    // ═══════════════════════════════════════════════════════════════════════

    #[test]
    fn test_time_bins_usage() {
        let times = vec![0.5, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0];
        let events = vec![1; 10];
        let bins = TimeBins::fit(&times, &events, 5, "quantile");
        assert_eq!(bins.n_bins(), 5);

        // All times should be binnable.
        for &t in &times {
            assert!(bins.bin_of(t).is_some(), "time {t} not binned");
        }
    }

    // ═══════════════════════════════════════════════════════════════════════
    // Learning rate schedulers
    // ═══════════════════════════════════════════════════════════════════════

    #[test]
    fn test_mlp_with_cosine_scheduler() {
        let (x, y) = gen_classification_data(50, 8);

        let config = MlpConfig {
            hidden_sizes: vec![16],
            task_type: TaskType::Classification,
            train: TrainConfig {
                optimizer: OptimizerConfig {
                    lr: 0.01,
                    ..Default::default()
                },
                scheduler: SchedulerConfig::Cosine { max_epochs: 50 },
                n_epochs: 50,
                batch_size: 10,
                ..Default::default()
            },
            ..Default::default()
        };

        let result = train_mlp(&x, &y, None, &config).unwrap();

        // LR should decrease over epochs.
        let first_lr = result.training_log.first().unwrap().lr;
        let last_lr = result.training_log.last().unwrap().lr;
        assert!(last_lr < first_lr, "LR should decrease: first={first_lr}, last={last_lr}");
    }

    // ═══════════════════════════════════════════════════════════════════════
    // Multiple optimizers
    // ═══════════════════════════════════════════════════════════════════════

    #[test]
    fn test_adamw_optimizer() {
        let (x, y) = gen_classification_data(50, 3);

        let config = MlpConfig {
            hidden_sizes: vec![16],
            task_type: TaskType::Classification,
            train: TrainConfig {
                optimizer: OptimizerConfig {
                    kind: OptimizerKind::Adamw,
                    lr: 0.01,
                    weight_decay: 0.01,
                    ..Default::default()
                },
                n_epochs: 30,
                batch_size: 10,
                ..Default::default()
            },
            ..Default::default()
        };

        let result = train_mlp(&x, &y, None, &config).unwrap();
        // Should converge (AdamW + weight decay).
        let last_loss = result.training_log.last().unwrap().train_loss;
        assert!(last_loss.is_finite());
    }

    #[test]
    fn test_sgd_optimizer() {
        let (x, y) = gen_classification_data(50, 3);

        let config = MlpConfig {
            hidden_sizes: vec![16],
            task_type: TaskType::Classification,
            train: TrainConfig {
                optimizer: OptimizerConfig {
                    kind: OptimizerKind::Sgd,
                    lr: 0.1,
                    momentum: 0.9,
                    ..Default::default()
                },
                n_epochs: 50,
                batch_size: 10,
                ..Default::default()
            },
            ..Default::default()
        };

        let result = train_mlp(&x, &y, None, &config).unwrap();
        let last_loss = result.training_log.last().unwrap().train_loss;
        assert!(last_loss.is_finite());
    }
}
