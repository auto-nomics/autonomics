//! Phase 3 comprehensive E2E tests: AutoEncoder, VAE, DeepHit, RNN, Embed.

#[cfg(test)]
mod tests {
    use dl::*;
    use rand::SeedableRng;
    use rand::Rng;
    use rand_chacha::ChaCha8Rng;

    // ═══════════════════════════════════════════════════════════════════════
    // AutoEncoder pipeline
    // ═══════════════════════════════════════════════════════════════════════

    #[test]
    fn test_autoencoder_reduces_dimension() {
        let n = 50;
        let mut rng = ChaCha8Rng::seed_from_u64(42);
        let mut x_data = Vec::with_capacity(n * 6);
        for _ in 0..n {
            let a = rng.random::<f64>() * 3.0;
            let b = rng.random::<f64>() * 3.0;
            // 6 features, but only 2 degrees of freedom.
            x_data.extend_from_slice(&[a, b, a + b, a - b, a * 2.0, b * 0.5]);
        }
        let x = Tensor::from_rows(n, 6, &x_data);

        let config = AutoEncoderConfig {
            kind: AeKind::Autoencoder,
            encoder_sizes: vec![8],
            decoder_sizes: vec![8],
            latent_dim: 2,
            activation: Activation::Relu,
            dropout: 0.0,
            loss: AeLoss::Mse,
            beta: 1.0,
            kl_warmup_epochs: 5,
            train: TrainConfig {
                optimizer: OptimizerConfig {
                    kind: OptimizerKind::Adam,
                    lr: 0.01,
                    ..Default::default()
                },
                n_epochs: 15,
                batch_size: 10,
                ..Default::default()
            },
        };

        let mut result = train_autoencoder(&x, None, &config).unwrap();
        assert_eq!(result.latent.shape(), (n, 2));
        assert_eq!(result.reconstructed.shape(), (n, 6));

        // Latent representation should be finite.
        for i in 0..n {
            for j in 0..2 {
                assert!(result.latent.at(i, j).is_finite());
            }
        }
    }

    #[test]
    fn test_vae_pipeline() {
        let n = 40;
        let mut rng = ChaCha8Rng::seed_from_u64(7);
        let mut x_data = Vec::with_capacity(n * 4);
        for _ in 0..n {
            let a = rng.random::<f64>() * 3.0;
            let b = rng.random::<f64>() * 3.0;
            x_data.extend_from_slice(&[a, b, a + b, b - a]);
        }
        let x = Tensor::from_rows(n, 4, &x_data);

        let config = AutoEncoderConfig {
            kind: AeKind::Vae,
            encoder_sizes: vec![8],
            decoder_sizes: vec![8],
            latent_dim: 3,
            activation: Activation::Relu,
            dropout: 0.0,
            loss: AeLoss::Mse,
            beta: 0.5,
            kl_warmup_epochs: 3,
            train: TrainConfig {
                optimizer: OptimizerConfig {
                    kind: OptimizerKind::Adam,
                    lr: 0.01,
                    ..Default::default()
                },
                n_epochs: 10,
                batch_size: 8,
                ..Default::default()
            },
        };

        let mut result = train_autoencoder(&x, None, &config).unwrap();
        assert_eq!(result.latent.shape(), (n, 3));

        // Serialize/deserialize model.
        let json = serde_json::to_string(&result.model).unwrap();
        let mut restored: AutoEncoderModel = serde_json::from_str(&json).unwrap();

        // Predict latent on new data.
        let x_new = Tensor::from_rows(5, 4, &[1.0, 2.0, 3.0, 1.0, 0.5, 1.0, 1.5, 0.5,
                                              2.0, 3.0, 5.0, 1.0, 0.0, 0.0, 0.0, 0.0,
                                              3.0, 1.0, 4.0, -2.0]);
        let latent = predict_autoencoder_latent(&mut restored, &x_new);
        assert_eq!(latent.shape(), (5, 3));
    }

    #[test]
    fn test_autoencoder_reconstruction() {
        let n = 30;
        let mut rng = ChaCha8Rng::seed_from_u64(10);
        let mut x_data = Vec::with_capacity(n * 3);
        for _ in 0..n {
            let v = rng.random::<f64>() * 5.0;
            x_data.extend_from_slice(&[v, v * 2.0, v * 3.0]);
        }
        let x = Tensor::from_rows(n, 3, &x_data);

        let config = AutoEncoderConfig {
            kind: AeKind::Autoencoder,
            encoder_sizes: vec![4],
            decoder_sizes: vec![4],
            latent_dim: 1,
            activation: Activation::Relu,
            dropout: 0.0,
            loss: AeLoss::Mse,
            beta: 1.0,
            kl_warmup_epochs: 0,
            train: TrainConfig {
                optimizer: OptimizerConfig {
                    kind: OptimizerKind::Adam,
                    lr: 0.01,
                    ..Default::default()
                },
                n_epochs: 10,
                batch_size: 10,
                ..Default::default()
            },
        };

        let mut result = train_autoencoder(&x, None, &config).unwrap();
        let recon = predict_autoencoder_reconstruct(&mut result.model, &x);
        assert_eq!(recon.shape(), (n, 3));
        // Reconstruction should be finite.
        for i in 0..n {
            for j in 0..3 {
                assert!(recon.at(i, j).is_finite());
            }
        }
    }

    // ═══════════════════════════════════════════════════════════════════════
    // DeepHit pipeline
    // ═══════════════════════════════════════════════════════════════════════

    #[test]
    fn test_deephit_single_cause_pipeline() {
        let n = 80;
        let mut rng = ChaCha8Rng::seed_from_u64(42);
        let mut x_data = Vec::with_capacity(n * 3);
        let mut times = Vec::with_capacity(n);
        let mut events = Vec::with_capacity(n);

        for _ in 0..n {
            let x1 = rng.random::<f64>() * 4.0 - 2.0;
            let x2 = rng.random::<f64>() * 4.0 - 2.0;
            let x3 = rng.random::<f64>() * 2.0;
            let risk = x1 * x1 + x2 * x2;
            let u = rng.random::<f64>();
            let t = -u.ln() / (0.3 * risk.exp().max(0.1));
            x_data.extend_from_slice(&[x1, x2, x3]);
            if t < 5.0 {
                events.push(1);
                times.push(t.max(0.01));
            } else {
                events.push(0);
                times.push(5.0);
            }
        }
        let x = Tensor::from_rows(n, 3, &x_data);

        let config = DeepHitConfig {
            hidden_sizes: vec![16, 8],
            activation: Activation::Relu,
            dropout: 0.0,
            n_time_bins: 5,
            time_bins_method: "quantile".into(),
            n_causes: 1,
            loss_alpha: 1.0,
            loss_beta: 0.1,
            loss_gamma: 1.0,
            train: TrainConfig {
                optimizer: OptimizerConfig {
                    kind: OptimizerKind::Adam,
                    lr: 0.01,
                    ..Default::default()
                },
                n_epochs: 15,
                batch_size: 16,
                ..Default::default()
            },
        };

        let result = train_deephit(&x, &times, &events, None, &config).unwrap();
        assert_eq!(result.risk_scores.shape(), (n, 1));

        // Risk scores should be finite.
        for i in 0..n {
            assert!(result.risk_scores.at(i, 0).is_finite());
        }

        // C-index should be better than random.
        let risks: Vec<f64> = (0..n).map(|i| result.risk_scores.at(i, 0)).collect();
        let ci = c_index(&risks, &times, &events);
        assert!(ci > 0.52, "C-index too low: {ci:.3}");
    }

    #[test]
    fn test_deephit_competing_risks() {
        let n = 80;
        let mut rng = ChaCha8Rng::seed_from_u64(42);
        let mut x_data = Vec::with_capacity(n * 3);
        let mut times = Vec::with_capacity(n);
        let mut events = Vec::with_capacity(n);

        for _ in 0..n {
            let x1 = rng.random::<f64>() * 4.0 - 2.0;
            let x2 = rng.random::<f64>() * 4.0 - 2.0;
            let x3 = rng.random::<f64>() * 2.0;
            let risk = x1 * x1 + x2 * x2;
            let u = rng.random::<f64>();
            let t = -u.ln() / (0.3 * risk.exp().max(0.1));
            x_data.extend_from_slice(&[x1, x2, x3]);
            if t < 5.0 {
                // Two competing causes.
                let cause = if x3 > 1.0 { 2 } else { 1 };
                events.push(cause);
                times.push(t.max(0.01));
            } else {
                events.push(0);
                times.push(5.0);
            }
        }
        let x = Tensor::from_rows(n, 3, &x_data);

        let config = DeepHitConfig {
            hidden_sizes: vec![16, 8],
            activation: Activation::Relu,
            dropout: 0.0,
            n_time_bins: 5,
            time_bins_method: "quantile".into(),
            n_causes: 2, // competing risks
            loss_alpha: 1.0,
            loss_beta: 0.1,
            loss_gamma: 1.0,
            train: TrainConfig {
                optimizer: OptimizerConfig {
                    kind: OptimizerKind::Adam,
                    lr: 0.01,
                    ..Default::default()
                },
                n_epochs: 10,
                batch_size: 16,
                ..Default::default()
            },
        };

        let result = train_deephit(&x, &times, &events, None, &config).unwrap();
        assert_eq!(result.risk_scores.shape(), (n, 1));
        assert_eq!(result.model.cause_heads.len(), 2); // two cause-specific heads

        // Serialize/deserialize.
        let json = serde_json::to_string(&result.model).unwrap();
        let restored: DeepHitModel = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.cause_heads.len(), 2);
    }

    // ═══════════════════════════════════════════════════════════════════════
    // RNN/LSTM/GRU pipeline
    // ═══════════════════════════════════════════════════════════════════════

    fn make_sequence_data(n: usize, seq_len: usize, n_feat: usize) -> (Tensor, Tensor) {
        let mut rng = ChaCha8Rng::seed_from_u64(42);
        let mut x = Vec::with_capacity(n * n_feat * seq_len);
        let mut y = Vec::with_capacity(n);

        for _ in 0..n {
            let mut increasing = true;
            for f in 0..n_feat {
                let trend = rng.random::<f64>() * 4.0 - 2.0;
                if trend < 0.0 { increasing = false; }
                for t in 0..seq_len {
                    x.push(trend * (t as f64 + 1.0) / seq_len as f64 + rng.random::<f64>() * 0.1);
                }
            }
            y.push(if increasing { 1.0 } else { 0.0 });
        }
        (Tensor::from_rows(n, n_feat * seq_len, &x), Tensor::from_rows(n, 1, &y))
    }

    #[test]
    fn test_lstm_classification() {
        let (x, y) = make_sequence_data(40, 4, 2);
        let config = RnnConfig {
            cell_type: CellType::Lstm,
            hidden_size: 4,
            n_layers: 1,
            bidirectional: false,
            dropout: 0.0,
            pooling: SeqPooling::Last,
            task_type: TaskType::Classification,
            train: TrainConfig {
                optimizer: OptimizerConfig {
                    kind: OptimizerKind::Adam,
                    lr: 0.01,
                    ..Default::default()
                },
                n_epochs: 5,
                batch_size: 8,
                ..Default::default()
            },
        };

        let result = train_rnn(&x, &y, None, &config, 2, 0, 4).unwrap();
        assert_eq!(result.predictions.shape(), (40, 1));
        for i in 0..40 {
            assert!(result.predictions.at(i, 0).is_finite());
        }
    }

    #[test]
    fn test_gru_classification() {
        let (x, y) = make_sequence_data(40, 4, 2);
        let config = RnnConfig {
            cell_type: CellType::Gru,
            hidden_size: 4,
            n_layers: 1,
            bidirectional: false,
            dropout: 0.0,
            pooling: SeqPooling::Mean,
            task_type: TaskType::Classification,
            train: TrainConfig {
                optimizer: OptimizerConfig {
                    kind: OptimizerKind::Adam,
                    lr: 0.01,
                    ..Default::default()
                },
                n_epochs: 5,
                batch_size: 8,
                ..Default::default()
            },
        };

        let result = train_rnn(&x, &y, None, &config, 2, 0, 4).unwrap();
        assert_eq!(result.predictions.shape(), (40, 1));
    }

    #[test]
    fn test_rnn_bidirectional() {
        let (x, y) = make_sequence_data(40, 4, 2);
        let config = RnnConfig {
            cell_type: CellType::Lstm,
            hidden_size: 4,
            n_layers: 1,
            bidirectional: true,
            dropout: 0.0,
            pooling: SeqPooling::Max,
            task_type: TaskType::Classification,
            train: TrainConfig {
                optimizer: OptimizerConfig {
                    kind: OptimizerKind::Adam,
                    lr: 0.01,
                    ..Default::default()
                },
                n_epochs: 5,
                batch_size: 8,
                ..Default::default()
            },
        };

        let result = train_rnn(&x, &y, None, &config, 2, 0, 4).unwrap();
        assert_eq!(result.predictions.shape(), (40, 1));
    }

    #[test]
    fn test_rnn_serialization() {
        let (x, y) = make_sequence_data(30, 4, 2);
        let config = RnnConfig {
            cell_type: CellType::Lstm,
            hidden_size: 4,
            n_layers: 1,
            bidirectional: false,
            dropout: 0.0,
            pooling: SeqPooling::Last,
            task_type: TaskType::Classification,
            train: TrainConfig {
                optimizer: OptimizerConfig {
                    kind: OptimizerKind::Adam,
                    lr: 0.01,
                    ..Default::default()
                },
                n_epochs: 3,
                batch_size: 8,
                ..Default::default()
            },
        };

        let result = train_rnn(&x, &y, None, &config, 2, 0, 4).unwrap();

        let json = serde_json::to_string(&result.model).unwrap();
        let restored: RnnModel = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.n_seq_features, 2);
        assert_eq!(restored.seq_length, 4);

        // Predict with restored model.
        let mut model = restored;
        let probs = predict_rnn(&mut model, &x);
        assert_eq!(probs.shape(), (30, 1));
    }

    // ═══════════════════════════════════════════════════════════════════════
    // Embed extraction
    // ═══════════════════════════════════════════════════════════════════════

    #[test]
    fn test_autoencoder_embed_extraction() {
        let n = 30;
        let mut rng = ChaCha8Rng::seed_from_u64(42);
        let mut x_data = Vec::new();
        for _ in 0..n {
            let a = rng.random::<f64>() * 3.0;
            let b = rng.random::<f64>() * 3.0;
            x_data.extend_from_slice(&[a, b, a + b]);
        }
        let x = Tensor::from_rows(n, 3, &x_data);

        let config = AutoEncoderConfig {
            kind: AeKind::Autoencoder,
            encoder_sizes: vec![4],
            decoder_sizes: vec![4],
            latent_dim: 2,
            activation: Activation::Relu,
            dropout: 0.0,
            loss: AeLoss::Mse,
            beta: 1.0,
            kl_warmup_epochs: 0,
            train: TrainConfig {
                optimizer: OptimizerConfig {
                    kind: OptimizerKind::Adam,
                    lr: 0.01,
                    ..Default::default()
                },
                n_epochs: 5,
                batch_size: 10,
                ..Default::default()
            },
        };

        let mut result = train_autoencoder(&x, None, &config).unwrap();

        // Extract latent from trained model.
        let latent = predict_autoencoder_latent(&mut result.model, &x);
        assert_eq!(latent.shape(), (n, 2));
        for i in 0..n {
            for j in 0..2 {
                assert!(latent.at(i, j).is_finite());
            }
        }
    }

    #[test]
    fn test_mlp_embed_extraction() {
        // Train MLP, then extract hidden layer activations.
        let n = 40;
        let mut rng = ChaCha8Rng::seed_from_u64(42);
        let mut x_data = Vec::new();
        let mut y_data = Vec::new();
        for _ in 0..n {
            let a = rng.random::<f64>() * 4.0 - 2.0;
            let b = rng.random::<f64>() * 4.0 - 2.0;
            x_data.extend_from_slice(&[a, b]);
            y_data.push(if a + b > 0.0 { 1.0 } else { 0.0 });
        }
        let x = Tensor::from_rows(n, 2, &x_data);
        let y = Tensor::from_rows(n, 1, &y_data);

        let config = MlpConfig {
            hidden_sizes: vec![8, 4],
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
                n_epochs: 10,
                batch_size: 10,
                ..Default::default()
            },
        };

        let result = train_mlp(&x, &y, None, &config).unwrap();
        let mut model = result.model;

        // Forward to last hidden layer.
        let x_scaled = model.scaler.as_ref().map(|s| s.transform(&x)).unwrap_or_else(|| x.clone());
        // Run through all layers except the last (output) layer.
        let n_hidden = model.layers.len() - 1;
        let mut h = x_scaled;
        for l in 0..n_hidden {
            h = model.layers[l].forward(&h);
            h = model.activations[l].forward(&h);
        }

        // Should have the hidden layer dimension.
        assert_eq!(h.shape(), (n, 4)); // last hidden is 4
        for i in 0..n {
            for j in 0..4 {
                assert!(h.at(i, j).is_finite());
            }
        }
    }

    // ═══════════════════════════════════════════════════════════════════════
    // Cross-architecture comparison
    // ═══════════════════════════════════════════════════════════════════════

    #[test]
    fn test_mlp_vs_deepsurv_on_survival_data() {
        let n = 100;
        let mut rng = ChaCha8Rng::seed_from_u64(42);
        let mut x_data = Vec::with_capacity(n * 4);
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
            x_data.extend_from_slice(&[x1, x2, x3, x4]);
            if t < 5.0 {
                events.push(1);
                times.push(t.max(0.01));
            } else {
                events.push(0);
                times.push(5.0);
            }
        }
        let x = Tensor::from_rows(n, 4, &x_data);

        // DeepSurv.
        let ds_config = DeepSurvConfig {
            hidden_sizes: vec![32, 16],
            activation: Activation::Relu,
            dropout: 0.1,
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
        let ds_result = train_deepsurv(&x, &times, &events, None, &ds_config).unwrap();
        let ds_risks: Vec<f64> = (0..n).map(|i| ds_result.risk_scores.at(i, 0)).collect();
        let ds_ci = c_index(&ds_risks, &times, &events);

        // DeepHit.
        let dh_config = DeepHitConfig {
            hidden_sizes: vec![32, 16],
            activation: Activation::Relu,
            dropout: 0.0,
            n_time_bins: 5,
            time_bins_method: "quantile".into(),
            n_causes: 1,
            loss_alpha: 1.0,
            loss_beta: 0.1,
            loss_gamma: 1.0,
            train: TrainConfig {
                optimizer: OptimizerConfig {
                    kind: OptimizerKind::Adam,
                    lr: 0.01,
                    ..Default::default()
                },
                n_epochs: 15,
                batch_size: 16,
                ..Default::default()
            },
        };
        let dh_result = train_deephit(&x, &times, &events, None, &dh_config).unwrap();
        let dh_risks: Vec<f64> = (0..n).map(|i| dh_result.risk_scores.at(i, 0)).collect();
        let dh_ci = c_index(&dh_risks, &times, &events);

        // Both should beat random.
        assert!(ds_ci > 0.6, "DeepSurv C-index too low: {ds_ci:.3}");
        assert!(dh_ci > 0.52, "DeepHit C-index too low: {dh_ci:.3}");

        // DeepSurv should generally outperform DeepHit with fewer epochs.
        // (not strictly required, just a sanity check)
        println!("DeepSurv C-index: {ds_ci:.3}, DeepHit C-index: {dh_ci:.3}");
    }

    // ═══════════════════════════════════════════════════════════════════════
    // Full artifact roundtrip for all architectures
    // ═══════════════════════════════════════════════════════════════════════

    #[test]
    fn test_all_architectures_artifact_roundtrip() {
        let architectures = vec![
            Architecture::Mlp,
            Architecture::Deepsurv,
            Architecture::Transformer,
            Architecture::Autoencoder,
            Architecture::Deephit,
            Architecture::Rnn,
        ];

        for arch in architectures {
            let artifact = DLModelArtifact {
                backend: "faer".into(),
                architecture: arch,
                task_type: ArtifactTaskType::Classification,
                checkpoint_json: "{}".into(),
                feature_names: vec!["x1".into()],
                label_column: Some("y".into()),
                time_column: None,
                event_column: None,
                scaler_json: None,
                training_meta: TrainingMeta::default(),
            };
            let bytes = artifact.to_bytes().unwrap();
            let restored = DLModelArtifact::from_bytes(&bytes).unwrap();
            assert_eq!(restored.architecture, arch);
            assert_eq!(restored.backend, "faer");
        }
    }
}
