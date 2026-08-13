//! Full pipeline integration tests (Burn backend).

#[cfg(test)]
mod tests {
    use dl::*;
    use rand::Rng;
    use rand::SeedableRng;
    use rand_chacha::ChaCha8Rng;

    fn gen_classification_data(n: usize, seed: u64) -> (Tensor, Tensor) {
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

    fn gen_survival_data(n: usize, seed: u64) -> (Tensor, Vec<f64>, Vec<usize>) {
        let mut rng = ChaCha8Rng::seed_from_u64(seed);
        let mut x = Vec::with_capacity(n * 4);
        let mut times = Vec::with_capacity(n);
        let mut events = Vec::with_capacity(n);
        for _ in 0..n {
            let x1 = rng.random::<f64>() * 4.0 - 2.0;
            let x2 = rng.random::<f64>() * 4.0 - 2.0;
            let risk = x1 * x1 + x2 * x2;
            let u = rng.random::<f64>();
            let t = -u.ln() / (0.3 * risk.exp().max(0.1));
            x.extend_from_slice(&[x1, x2, rng.random::<f64>() * 2.0, rng.random::<f64>() * 2.0]);
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
    fn test_mlp_classification_pipeline() {
        let (x_train, y_train) = gen_classification_data(100, 42);
        let (x_test, y_test) = gen_classification_data(50, 99);
        let config = MlpConfig {
            hidden_sizes: vec![16, 8],
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
                batch_size: 16,
                ..Default::default()
            },
        };
        let result = train_mlp(&x_train, &y_train, None, &config).unwrap();
        let json = serde_json::to_string(&result.model).unwrap();
        let mut restored: MlpModel = serde_json::from_str(&json).unwrap();
        let probs = predict_mlp(&mut restored, &x_test);
        let correct = (0..50)
            .filter(|&i| {
                let p = probs.at(i, 0);
                ((if p > 0.5 { 1.0 } else { 0.0 }) - y_test.at(i, 0)).abs() < 0.5
            })
            .count();
        let acc = correct as f64 / 50.0;
        assert!(acc > 0.65, "test accuracy too low: {acc:.2}");
    }

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
                n_epochs: 100,
                batch_size: 16,
                ..Default::default()
            },
        };
        let result = train_mlp(&x, &y, None, &config).unwrap();
        let preds = result.predictions;
        let mse: f64 = (0..n)
            .map(|i| (preds.at(i, 0) - y_data[i]).powi(2))
            .sum::<f64>()
            / n as f64;
        assert!(mse < 5.0, "regression MSE too high: {mse:.3}");
    }

    #[test]
    fn test_deepsurv_pipeline_with_metrics() {
        let (x, times, events) = gen_survival_data(100, 42);
        let (x_test, t_test, e_test) = gen_survival_data(50, 99);
        let config = DeepSurvConfig {
            hidden_sizes: vec![32, 16],
            activation: Activation::Relu,
            dropout: 0.0,
            train: TrainConfig {
                optimizer: OptimizerConfig {
                    kind: OptimizerKind::Adam,
                    lr: 0.005,
                    ..Default::default()
                },
                n_epochs: 50,
                batch_size: 16,
                ..Default::default()
            },
        };
        let result = train_deepsurv(&x, &times, &events, None, &config).unwrap();
        let train_risks: Vec<f64> = (0..100).map(|i| result.risk_scores.at(i, 0)).collect();
        let train_ci = c_index(&train_risks, &times, &events);
        assert!(train_ci > 0.6, "training C-index too low: {train_ci:.3}");
        let json = serde_json::to_string(&result.model).unwrap();
        let mut restored: DeepSurvModel = serde_json::from_str(&json).unwrap();
        let test_risks = predict_deepsurv(&mut restored, &x_test);
        let test_risks_vec: Vec<f64> = (0..50).map(|i| test_risks.at(i, 0)).collect();
        let test_ci = c_index(&test_risks_vec, &t_test, &e_test);
        assert!(test_ci > 0.55, "test C-index too low: {test_ci:.3}");
    }

    #[test]
    fn test_artifact_persistence() {
        let (x, y) = gen_classification_data(50, 1);
        let config = MlpConfig {
            hidden_sizes: vec![8],
            task_type: TaskType::Classification,
            train: TrainConfig {
                n_epochs: 10,
                batch_size: 10,
                ..Default::default()
            },
            ..Default::default()
        };
        let result = train_mlp(&x, &y, None, &config).unwrap();
        let checkpoint = serde_json::to_string(&result.model).unwrap();
        let artifact = DLModelArtifact {
            backend: "burn".into(),
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
        let bytes = artifact.to_bytes().unwrap();
        let restored = DLModelArtifact::from_bytes(&bytes).unwrap();
        assert_eq!(restored.architecture, Architecture::Mlp);
        assert_eq!(restored.backend, "burn");
    }

    #[test]
    fn test_mlp_with_early_stopping() {
        let (x_train, y_train) = gen_classification_data(60, 10);
        let (x_val, y_val) = gen_classification_data(20, 20);
        let config = MlpConfig {
            hidden_sizes: vec![32, 16],
            task_type: TaskType::Classification,
            train: TrainConfig {
                n_epochs: 200,
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
        let result = train_mlp(&x_train, &y_train, Some((&x_val, &y_val)), &config).unwrap();
        assert!(result.training_log.len() <= 205);
        assert!(result.training_log.iter().any(|l| l.val_loss.is_some()));
    }

    #[test]
    fn test_mlp_with_gradient_clipping() {
        let (x, y) = gen_classification_data(50, 5);
        let config = MlpConfig {
            hidden_sizes: vec![16, 8],
            task_type: TaskType::Classification,
            train: TrainConfig {
                n_epochs: 20,
                batch_size: 10,
                gradient_clip_norm: Some(1.0),
                ..Default::default()
            },
            ..Default::default()
        };
        let result = train_mlp(&x, &y, None, &config).unwrap();
        for log in &result.training_log {
            assert!(log.train_loss.is_finite());
        }
    }

    #[test]
    fn test_time_bins_usage() {
        let times: Vec<f64> = (1..=10).map(|i| i as f64).collect();
        let events = vec![1; 10];
        let bins = TimeBins::fit(&times, &events, 5, "quantile");
        assert_eq!(bins.n_bins(), 5);
        for &t in &times {
            assert!(bins.bin_of(t).is_some());
        }
    }

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
                scheduler: SchedulerConfig::Cosine { max_epochs: 30 },
                n_epochs: 30,
                batch_size: 10,
                ..Default::default()
            },
            ..Default::default()
        };
        let result = train_mlp(&x, &y, None, &config).unwrap();
        let first_lr = result.training_log.first().unwrap().lr;
        let last_lr = result.training_log.last().unwrap().lr;
        assert!(last_lr < first_lr || result.training_log.len() < 2);
    }

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
                n_epochs: 20,
                batch_size: 10,
                ..Default::default()
            },
            ..Default::default()
        };
        let result = train_mlp(&x, &y, None, &config).unwrap();
        assert!(result.training_log.last().unwrap().train_loss.is_finite());
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
                n_epochs: 30,
                batch_size: 10,
                ..Default::default()
            },
            ..Default::default()
        };
        let result = train_mlp(&x, &y, None, &config).unwrap();
        assert!(result.training_log.last().unwrap().train_loss.is_finite());
    }
}
