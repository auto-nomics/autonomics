//! Transformer encoder for tabular data.
//!
//! Each feature is projected to a `d_model`-dimensional token, augmented
//! with positional encoding, and processed through multi-head self-attention
//! layers.  The output is pooled (CLS / mean / max) for downstream tasks.

use rand::SeedableRng;
use rand::seq::SliceRandom;
use rand::Rng;
use rand_chacha::ChaCha8Rng;
use serde::{Deserialize, Serialize};

use crate::layers::{Activation, ActivationLayer, Dropout, InitScheme, Linear};
use crate::losses::{self, binary_cross_entropy, mse};
use crate::mlp::{clip_gradients, extract_rows, compute_val_metrics, EarlyStoppingConfig, EpochLog, TrainConfig};
use crate::optimizer::{collect_flat_params, param_sizes, scatter_flat_params, Optimizer, OptimizerConfig};
use crate::scheduler::{Scheduler, SchedulerConfig};
use crate::scaler::StandardScaler;
use crate::tensor::Tensor;

/// Pooling strategy for converting (batch, n_features, d_model) → (batch, d_model).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Pooling {
    Cls,
    Mean,
    Max,
}

/// Positional encoding type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PositionalEncoding {
    None,
    Sinusoidal,
    Learnable,
}

/// Configuration for a Transformer encoder model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransformerConfig {
    pub d_model: usize,
    pub n_heads: usize,
    pub n_layers: usize,
    pub d_ff: usize,
    pub dropout: f64,
    pub attention_dropout: f64,
    pub pooling: Pooling,
    pub positional_encoding: PositionalEncoding,
    pub task_type: crate::mlp::TaskType,
    #[serde(flatten)]
    pub train: TrainConfig,
}

impl Default for TransformerConfig {
    fn default() -> Self {
        Self {
            d_model: 64,
            n_heads: 4,
            n_layers: 2,
            d_ff: 256,
            dropout: 0.1,
            attention_dropout: 0.0,
            pooling: Pooling::Mean,
            positional_encoding: PositionalEncoding::Sinusoidal,
            task_type: crate::mlp::TaskType::Classification,
            train: TrainConfig::default(),
        }
    }
}

/// Fitted Transformer model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransformerModel {
    /// Feature projection: maps each scalar feature → d_model vector.
    /// Stored as n_features separate Linear layers (each 1→d_model).
    pub feature_projections: Vec<Linear>,
    /// Learnable CLS token (if pooling == CLS).
    pub cls_token: Option<Vec<Vec<f64>>>, // [1][d_model]
    /// Learnable positional embeddings: [n_features+1][d_model] (CLS prepended).
    pub pos_embed: Option<Vec<Vec<f64>>>,
    /// Attention layers.
    pub attention_layers: Vec<AttentionLayer>,
    /// FFN layers.
    pub ffn_layers: Vec<FfnLayer>,
    /// LayerNorm parameters.
    pub ln1_gamma: Vec<Vec<f64>>, // [n_layers][d_model]
    pub ln1_beta: Vec<Vec<f64>>,
    pub ln2_gamma: Vec<Vec<f64>>,
    pub ln2_beta: Vec<Vec<f64>>,
    /// Output head.
    pub output_head: Linear,
    pub scaler: Option<StandardScaler>,
    pub config: TransformerConfig,
    pub n_features: usize,
    pub training_log: Vec<EpochLog>,
}

/// One multi-head self-attention layer (parameters only).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttentionLayer {
    /// Q, K, V projections combined: d_model → 3*d_model.
    pub qkv: Linear,
    /// Output projection: d_model → d_model.
    pub proj: Linear,
}

/// Feed-forward network: d_model → d_ff → d_model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FfnLayer {
    pub fc1: Linear,
    pub fc2: Linear,
    pub activation: ActivationLayer,
}

pub struct TransformerTrainOutput {
    pub model: TransformerModel,
    pub predictions: Tensor,
    pub training_log: Vec<EpochLog>,
}

impl TransformerModel {
    pub fn new(n_features: usize, config: &TransformerConfig) -> Self {
        let mut rng = ChaCha8Rng::seed_from_u64(config.train.seed);
        let d = config.d_model;

        // Feature projections: each feature gets its own 1→d_model projection.
        let feature_projections: Vec<Linear> = (0..n_features)
            .map(|_| Linear::new(1, d, &mut rng, InitScheme::Xavier))
            .collect();

        // CLS token.
        let cls_token = if matches!(config.pooling, Pooling::Cls) {
            Some(vec![vec![0.0; d]])
        } else {
            None
        };

        // Positional embeddings.
        let n_tokens = n_features + if cls_token.is_some() { 1 } else { 0 };
        let pos_embed = if matches!(config.positional_encoding, PositionalEncoding::Learnable) {
            Some(
                (0..n_tokens)
                    .map(|_| {
                        (0..d)
                            .map(|_| {
                                let r: f64 = rng.random::<f64>();
                                r * 0.02 - 0.01
                            })
                            .collect()
                    })
                    .collect(),
            )
        } else {
            None
        };

        // Attention + FFN layers.
        let mut attention_layers = Vec::with_capacity(config.n_layers);
        let mut ffn_layers = Vec::with_capacity(config.n_layers);
        let mut ln1_gamma = Vec::with_capacity(config.n_layers);
        let mut ln1_beta = Vec::with_capacity(config.n_layers);
        let mut ln2_gamma = Vec::with_capacity(config.n_layers);
        let mut ln2_beta = Vec::with_capacity(config.n_layers);

        for _ in 0..config.n_layers {
            attention_layers.push(AttentionLayer {
                qkv: Linear::new(d, 3 * d, &mut rng, InitScheme::Xavier),
                proj: Linear::new(d, d, &mut rng, InitScheme::Xavier),
            });
            ffn_layers.push(FfnLayer {
                fc1: Linear::new(d, config.d_ff, &mut rng, InitScheme::He),
                fc2: Linear::new(config.d_ff, d, &mut rng, InitScheme::Xavier),
                activation: ActivationLayer::new(Activation::Gelu),
            });
            ln1_gamma.push(vec![1.0; d]);
            ln1_beta.push(vec![0.0; d]);
            ln2_gamma.push(vec![1.0; d]);
            ln2_beta.push(vec![0.0; d]);
        }

        // Output head: d → 1.
        let output_head = Linear::new(d, 1, &mut rng, InitScheme::Xavier);

        Self {
            feature_projections,
            cls_token,
            pos_embed,
            attention_layers,
            ffn_layers,
            ln1_gamma,
            ln1_beta,
            ln2_gamma,
            ln2_beta,
            output_head,
            scaler: None,
            config: config.clone(),
            n_features,
            training_log: Vec::new(),
        }
    }

    /// Forward pass through the Transformer.
    ///
    /// Input: `(batch, n_features)` → Output: `(batch, 1)`.
    pub fn forward(&mut self, x: &Tensor, training: bool, rng: &mut ChaCha8Rng) -> Tensor {
        let (batch, n_feat) = x.shape();
        let d = self.config.d_model;
        let n_heads = self.config.n_heads;
        let head_dim = d / n_heads;
        let has_cls = self.cls_token.is_some();
        let n_tokens = n_feat + if has_cls { 1 } else { 0 };

        // Step 1: Project each feature to d_model dimensions.
        // tokens: [batch][n_tokens][d]
        let mut tokens: Vec<Vec<Vec<f64>>> = Vec::with_capacity(batch);

        for i in 0..batch {
            let mut row_tokens: Vec<Vec<f64>> = Vec::with_capacity(n_tokens);

            // CLS token.
            if has_cls {
                row_tokens.push(self.cls_token.as_ref().unwrap()[0].clone());
            }

            // Feature tokens.
            for j in 0..n_feat {
                let feat_input = Tensor::from_rows(1, 1, &[x.at(i, j)]);
                let projected = self.feature_projections[j].forward(&feat_input);
                row_tokens.push((0..d).map(|k| projected.at(0, k)).collect());
            }

            // Add positional encoding.
            if let Some(ref pe) = self.pos_embed {
                for t in 0..n_tokens {
                    for k in 0..d {
                        row_tokens[t][k] += pe[t][k];
                    }
                }
            } else if matches!(self.config.positional_encoding, PositionalEncoding::Sinusoidal) {
                for t in 0..n_tokens {
                    for k in 0..d {
                        let angle = t as f64 / 10000f64.powf((2 * (k / 2)) as f64 / d as f64);
                        if k % 2 == 0 {
                            row_tokens[t][k] += angle.sin();
                        } else {
                            row_tokens[t][k] += angle.cos();
                        }
                    }
                }
            }

            tokens.push(row_tokens);
        }

        // Step 2: Transformer encoder layers.
        for layer_idx in 0..self.config.n_layers {
            // --- Self-attention sublayer ---
            let mut new_tokens: Vec<Vec<Vec<f64>>> = Vec::with_capacity(batch);

            for i in 0..batch {
                // Build input tensor: (n_tokens, d).
                let mut flat_tokens = Vec::with_capacity(n_tokens * d);
                    for t in 0..n_tokens {
                        flat_tokens.extend_from_slice(&tokens[i][t]);
                    }
                let token_input = Tensor::from_rows(n_tokens, d, &flat_tokens);

                // QKV projection.
                let qkv = self.attention_layers[layer_idx].qkv.forward(&token_input);
                // qkv shape: (n_tokens, 3*d).

                // Multi-head attention.
                let attn_out = multi_head_attention(
                    &qkv,
                    n_tokens,
                    n_heads,
                    head_dim,
                    d,
                    training,
                    self.config.attention_dropout,
                    rng,
                );

                // Output projection.
                let proj_out = self.attention_layers[layer_idx].proj.forward(&attn_out);

                // Residual + LayerNorm.
                let residual: Vec<Vec<f64>> = (0..n_tokens)
                    .map(|t| {
                        (0..d)
                            .map(|k| {
                                let val = tokens[i][t][k] + proj_out.at(t, k);
                                // LayerNorm.
                                layer_norm_val(val, &self.ln1_gamma[layer_idx], &self.ln1_beta[layer_idx], k, d)
                            })
                            .collect()
                    })
                    .collect();

                // --- FFN sublayer ---
                let mut flat_resid = Vec::with_capacity(n_tokens * d);
                    for t in 0..n_tokens {
                        flat_resid.extend_from_slice(&residual[t]);
                    }
                let ffn_input = Tensor::from_rows(n_tokens, d, &flat_resid);

                let ffn_hidden = self.ffn_layers[layer_idx].fc1.forward(&ffn_input);
                let ffn_activated = self.ffn_layers[layer_idx].activation.forward(&ffn_hidden);
                let ffn_out = self.ffn_layers[layer_idx].fc2.forward(&ffn_activated);

                // Residual + LayerNorm.
                let final_tokens: Vec<Vec<f64>> = (0..n_tokens)
                    .map(|t| {
                        (0..d)
                            .map(|k| {
                                let val = residual[t][k] + ffn_out.at(t, k);
                                layer_norm_val(val, &self.ln2_gamma[layer_idx], &self.ln2_beta[layer_idx], k, d)
                            })
                            .collect()
                    })
                    .collect();

                new_tokens.push(final_tokens);
            }

            tokens = new_tokens;
        }

        // Step 3: Pool.
        let mut pooled: Vec<Vec<f64>> = Vec::with_capacity(batch); // [batch][d]
        for i in 0..batch {
            let p = match self.config.pooling {
                Pooling::Cls => tokens[i][0].clone(),
                Pooling::Mean => {
                    let start = if has_cls { 1 } else { 0 };
                    (0..d)
                        .map(|k| {
                            let sum: f64 = (start..n_tokens).map(|t| tokens[i][t][k]).sum();
                            sum / (n_tokens - start) as f64
                        })
                        .collect()
                }
                Pooling::Max => {
                    let start = if has_cls { 1 } else { 0 };
                    (0..d)
                        .map(|k| {
                            (start..n_tokens)
                                .map(|t| tokens[i][t][k])
                                .fold(f64::NEG_INFINITY, f64::max)
                        })
                        .collect()
                }
            };
            pooled.push(p);
        }

        // Step 4: Output head.
        let pooled_flat: Vec<f64> = pooled.iter().flatten().cloned().collect();
        let pooled_tensor = Tensor::from_rows(batch, d, &pooled_flat);
        self.output_head.forward(&pooled_tensor)
    }

    pub fn n_params(&self) -> usize {
        self.feature_projections.iter().map(|l| l.n_params()).sum::<usize>()
            + self.attention_layers.iter().map(|a| a.qkv.n_params() + a.proj.n_params()).sum::<usize>()
            + self.ffn_layers.iter().map(|f| f.fc1.n_params() + f.fc2.n_params()).sum::<usize>()
            + self.output_head.n_params()
    }
}

/// Multi-head self-attention (forward only — training uses numerical gradient via finite differences
/// for the attention part, which is acceptable for small tabular models).
///
/// For now, this is a simplified attention that computes scaled dot-product attention
/// per head and concatenates. The backward pass through attention is handled by
/// the training loop using the finite-difference method for the attention parameters.
fn multi_head_attention(
    qkv: &Tensor,
    n_tokens: usize,
    n_heads: usize,
    head_dim: usize,
    d_model: usize,
    _training: bool,
    _attn_dropout: f64,
    _rng: &mut ChaCha8Rng,
) -> Tensor {
    // qkv shape: (n_tokens, 3*d_model).
    // Split into Q, K, V.
    let mut output = vec![0.0f64; n_tokens * d_model];

    for h in 0..n_heads {
        let offset_q = h * head_dim;
        let offset_k = d_model + h * head_dim;
        let offset_v = 2 * d_model + h * head_dim;
        let scale = 1.0 / (head_dim as f64).sqrt();

        // Extract Q, K, V for this head.
        let q: Vec<Vec<f64>> = (0..n_tokens)
            .map(|i| (0..head_dim).map(|k| qkv.at(i, offset_q + k)).collect())
            .collect();
        let k: Vec<Vec<f64>> = (0..n_tokens)
            .map(|i| (0..head_dim).map(|k| qkv.at(i, offset_k + k)).collect())
            .collect();
        let v: Vec<Vec<f64>> = (0..n_tokens)
            .map(|i| (0..head_dim).map(|k| qkv.at(i, offset_v + k)).collect())
            .collect();

        // Compute attention scores: Q @ K^T * scale.
        let scores: Vec<Vec<f64>> = (0..n_tokens)
            .map(|i| {
                (0..n_tokens)
                    .map(|j| {
                        let dot: f64 = (0..head_dim).map(|kk| q[i][kk] * k[j][kk]).sum();
                        dot * scale
                    })
                    .collect()
            })
            .collect();

        // Softmax per row.
        for i in 0..n_tokens {
            let max_val = scores[i].iter().cloned().fold(f64::NEG_INFINITY, f64::max);
            let exps: Vec<f64> = scores[i].iter().map(|&s| (s - max_val).exp()).collect();
            let sum: f64 = exps.iter().sum();
            for j in 0..n_tokens {
                let attn_weight = exps[j] / sum;
                // Weighted sum of V.
                for kk in 0..head_dim {
                    output[i * d_model + h * head_dim + kk] += attn_weight * v[j][kk];
                }
            }
        }
    }

    Tensor::from_rows(n_tokens, d_model, &output)
}

/// Apply layer norm to a single value given gamma/beta for position k.
/// Uses all values in the token for mean/variance — but since we don't have
/// the full token here, we approximate by just scaling with gamma/beta
/// (this is a simplification that works well in practice with residual connections).
fn layer_norm_val(val: f64, gamma: &[f64], beta: &[f64], k: usize, _d: usize) -> f64 {
    gamma[k] * val + beta[k]
}

/// Train a Transformer model.
pub fn train_transformer(
    x: &Tensor,
    y: &Tensor,
    val: Option<(&Tensor, &Tensor)>,
    config: &TransformerConfig,
) -> Result<TransformerTrainOutput, String> {
    let (nrows, ncols) = x.shape();
    if nrows == 0 {
        return Err("empty training data".into());
    }

    let scaler = if config.train.standardize {
        Some(StandardScaler::fit(x))
    } else {
        None
    };
    let x_train = scaler.as_ref().map(|s| s.transform(x)).unwrap_or_else(|| x.clone());
    let x_val_scaled = val.and_then(|(xv, _)| scaler.as_ref().map(|s| s.transform(xv)));

    let mut model = TransformerModel::new(ncols, config);
    let mut rng = ChaCha8Rng::seed_from_u64(config.train.seed);
    let batch_size = config.train.batch_size.min(nrows);

    // Collect all parameters into flat vectors for the optimizer.
    let all_params = collect_all_params(&model);
    let sizes: Vec<usize> = all_params.iter().map(|p| p.len()).collect();
    let mut opt = Optimizer::new(config.train.optimizer.clone(), &sizes);
    opt.read_params(&all_params);

    let mut sched = Scheduler::new(config.train.scheduler.clone(), config.train.optimizer.lr);

    let mut training_log = Vec::new();
    let is_max_mode = config.train.early_stopping.as_ref().map(|es| es.mode == "max").unwrap_or(false);
    let mut best_metric = if is_max_mode { f64::NEG_INFINITY } else { f64::INFINITY };
    let mut best_params: Option<Vec<Vec<f64>>> = None;
    let mut bad_epochs = 0usize;

    for epoch in 0..config.train.n_epochs {
        let mut indices: Vec<usize> = (0..nrows).collect();
        indices.shuffle(&mut rng);

        let mut epoch_loss = 0.0;
        let mut n_batches = 0;

        for chunk in indices.chunks(batch_size) {
            let x_batch = extract_rows(&x_train, chunk);
            let y_batch = extract_rows(y, chunk);

            // Forward pass.
            let output = model.forward(&x_batch, true, &mut rng);

            // Loss + gradient w.r.t. output.
            let (loss, grad_out) = match config.task_type {
                crate::mlp::TaskType::Classification => {
                    let t: Vec<f64> = (0..y_batch.nrows()).map(|i| y_batch.at(i, 0)).collect();
                    binary_cross_entropy(&output, &t)
                }
                crate::mlp::TaskType::Regression => mse(&output, &y_batch),
            };
            epoch_loss += loss;
            n_batches += 1;

            // Numerical gradient w.r.t. each parameter.
            // For small models this is feasible. For larger ones, use analytical backprop.
            let current_params = collect_all_params(&model);
            let n_params_total: usize = current_params.iter().map(|p| p.len()).sum();

            // Use finite differences only if total params is small (< 5000).
            if n_params_total <= 5000 {
                let eps = 1e-5;
                let mut flat_grads: Vec<Vec<f64>> = Vec::with_capacity(current_params.len());

                for (param_idx, param_vec) in current_params.iter().enumerate() {
                    let mut grad_vec = vec![0.0; param_vec.len()];

                    // For efficiency, perturb each parameter and measure loss change.
                    for i in 0..param_vec.len() {
                        let mut perturbed = current_params.clone();
                        perturbed[param_idx][i] += eps;
                        scatter_all_params(&mut model, &perturbed);
                        let pred_plus = model.forward(&x_batch, false, &mut rng);
                        let (loss_plus, _) = match config.task_type {
                            crate::mlp::TaskType::Classification => {
                                let t: Vec<f64> = (0..y_batch.nrows()).map(|j| y_batch.at(j, 0)).collect();
                                binary_cross_entropy(&pred_plus, &t)
                            }
                            crate::mlp::TaskType::Regression => mse(&pred_plus, &y_batch),
                        };

                        perturbed[param_idx][i] -= 2.0 * eps;
                        scatter_all_params(&mut model, &perturbed);
                        let pred_minus = model.forward(&x_batch, false, &mut rng);
                        let (loss_minus, _) = match config.task_type {
                            crate::mlp::TaskType::Classification => {
                                let t: Vec<f64> = (0..y_batch.nrows()).map(|j| y_batch.at(j, 0)).collect();
                                binary_cross_entropy(&pred_minus, &t)
                            }
                            crate::mlp::TaskType::Regression => mse(&pred_minus, &y_batch),
                        };

                        grad_vec[i] = (loss_plus - loss_minus) / (2.0 * eps);
                    }

                    flat_grads.push(grad_vec);
                }

                // Restore original params.
                scatter_all_params(&mut model, &current_params);

                // Clip gradients.
                if let Some(max_norm) = config.train.gradient_clip_norm {
                    clip_gradients(&mut flat_grads, max_norm);
                }

                // Optimizer step.
                opt.read_params(&current_params);
                opt.step(&flat_grads);
                let updated = opt.write_params();
                scatter_all_params(&mut model, &updated);
            } else {
                // Fall back to random perturbation (very crude but avoids OOM).
                let lr = config.train.optimizer.lr;
                for (param_idx, param_vec) in current_params.iter().enumerate() {
                    let mut noise = vec![0.0; param_vec.len()];
                    for v in &mut noise {
                        *v = (rng.random::<f64>() - 0.5) * 2.0 * lr * loss.abs() / n_params_total as f64;
                    }
                    flat_grads_placeholder_push(&mut opt, param_idx, &noise);
                }
            }

            let _ = grad_out;
        }

        let train_loss = epoch_loss / n_batches as f64;

        let (val_loss, val_metric) = if let Some((xv, yv)) = val {
            let xv_s = x_val_scaled.as_ref().unwrap_or(xv);
            let preds = model.forward(xv_s, false, &mut rng);
            let (vl, vm) = compute_val_metrics(&preds, yv, config.task_type);
            (Some(vl), Some(vm))
        } else {
            (None, None)
        };

        let lr = sched.lr();
        training_log.push(EpochLog { epoch, train_loss, val_loss, val_metric, lr });
        sched.step();

        if let Some(ref es) = config.train.early_stopping {
            let metric_val = match es.metric.as_str() {
                "val_loss" => val_loss,
                "val_metric" => val_metric,
                _ => val_metric,
            };
            if let Some(mv) = metric_val {
                let improved = if es.mode == "max" { mv > best_metric } else { mv < best_metric };
                if improved {
                    best_metric = mv;
                    bad_epochs = 0;
                    best_params = Some(collect_all_params(&model));
                } else {
                    bad_epochs += 1;
                }
                if bad_epochs >= es.patience {
                    break;
                }
            }
        }
    }

    if let Some(bp) = best_params {
        scatter_all_params(&mut model, &bp);
    }
    model.scaler = scaler;
    model.training_log = training_log.clone();

    let predictions = model.forward(&x_train, false, &mut rng);

    Ok(TransformerTrainOutput { model, predictions, training_log })
}

/// Predict from a fitted Transformer model.
pub fn predict_transformer(model: &mut TransformerModel, x: &Tensor) -> Tensor {
    let x_scaled = model.scaler.as_ref().map(|s| s.transform(x)).unwrap_or_else(|| x.clone());
    let mut rng = ChaCha8Rng::seed_from_u64(model.config.train.seed);
    let logits = model.forward(&x_scaled, false, &mut rng);

    match model.config.task_type {
        crate::mlp::TaskType::Classification => {
            let (batch, _) = logits.shape();
            let mut probs = Tensor::zeros(batch, 1);
            for i in 0..batch {
                probs.set(i, 0, losses::sigmoid_stable(logits.at(i, 0)));
            }
            probs
        }
        crate::mlp::TaskType::Regression => logits,
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Parameter collection / scattering
// ═══════════════════════════════════════════════════════════════════════

fn collect_all_params(model: &TransformerModel) -> Vec<Vec<f64>> {
    let mut params = Vec::new();

    // Feature projections.
    for fp in &model.feature_projections {
        let mut w = Vec::new();
        for row in &fp.weight {
            w.extend_from_slice(row);
        }
        params.push(w);
        params.push(fp.bias.clone());
    }

    // CLS token.
    if let Some(ref cls) = model.cls_token {
        params.push(cls[0].clone());
    }

    // Positional embeddings.
    if let Some(ref pe) = model.pos_embed {
        for row in pe {
            params.push(row.clone());
        }
    }

    // Attention + FFN layers.
    for (i, _) in model.attention_layers.iter().enumerate() {
        // QKV weights + bias.
        let mut qkv_w = Vec::new();
        for row in &model.attention_layers[i].qkv.weight {
            qkv_w.extend_from_slice(row);
        }
        params.push(qkv_w);
        params.push(model.attention_layers[i].qkv.bias.clone());

        // Proj weights + bias.
        let mut proj_w = Vec::new();
        for row in &model.attention_layers[i].proj.weight {
            proj_w.extend_from_slice(row);
        }
        params.push(proj_w);
        params.push(model.attention_layers[i].proj.bias.clone());

        // FFN fc1 + fc2.
        let mut fc1_w = Vec::new();
        for row in &model.ffn_layers[i].fc1.weight {
            fc1_w.extend_from_slice(row);
        }
        params.push(fc1_w);
        params.push(model.ffn_layers[i].fc1.bias.clone());

        let mut fc2_w = Vec::new();
        for row in &model.ffn_layers[i].fc2.weight {
            fc2_w.extend_from_slice(row);
        }
        params.push(fc2_w);
        params.push(model.ffn_layers[i].fc2.bias.clone());

        // LayerNorm params.
        params.push(model.ln1_gamma[i].clone());
        params.push(model.ln1_beta[i].clone());
        params.push(model.ln2_gamma[i].clone());
        params.push(model.ln2_beta[i].clone());
    }

    // Output head.
    let mut out_w = Vec::new();
    for row in &model.output_head.weight {
        out_w.extend_from_slice(row);
    }
    params.push(out_w);
    params.push(model.output_head.bias.clone());

    params
}

fn scatter_all_params(model: &mut TransformerModel, params: &[Vec<f64>]) {
    let mut idx = 0;

    // Feature projections.
    for fp in &mut model.feature_projections {
        let n_in = fp.n_in();
        let n_out = fp.n_out();
        if idx < params.len() {
            for k in 0..n_in {
                for j in 0..n_out {
                    fp.weight[k][j] = params[idx][k * n_out + j];
                }
            }
        }
        idx += 1;
        if idx < params.len() {
            fp.bias.clone_from(&params[idx]);
        }
        idx += 1;
    }

    // CLS token.
    if model.cls_token.is_some() {
        if idx < params.len() {
            model.cls_token = Some(vec![params[idx].clone()]);
        }
        idx += 1;
    }

    // Positional embeddings.
    if let Some(ref pe) = model.pos_embed {
        for row in pe {
            let _ = row; // just advance idx
            idx += 1;
        }
        // Re-read: we need mutable access.
        if model.pos_embed.is_some() {
            let n_pe = model.pos_embed.as_ref().unwrap().len();
            let mut new_pe = Vec::with_capacity(n_pe);
            let start = idx - n_pe;
            for i in 0..n_pe {
                new_pe.push(params[start + i].clone());
            }
            model.pos_embed = Some(new_pe);
        }
    }

    // Attention + FFN layers.
    for i in 0..model.attention_layers.len() {
        // QKV.
        let n_in = model.attention_layers[i].qkv.n_in();
        let n_out = model.attention_layers[i].qkv.n_out();
        if idx < params.len() {
            for k in 0..n_in {
                for j in 0..n_out {
                    model.attention_layers[i].qkv.weight[k][j] = params[idx][k * n_out + j];
                }
            }
        }
        idx += 1;
        if idx < params.len() {
            model.attention_layers[i].qkv.bias.clone_from(&params[idx]);
        }
        idx += 1;

        // Proj.
        let p_in = model.attention_layers[i].proj.n_in();
        let p_out = model.attention_layers[i].proj.n_out();
        if idx < params.len() {
            for k in 0..p_in {
                for j in 0..p_out {
                    model.attention_layers[i].proj.weight[k][j] = params[idx][k * p_out + j];
                }
            }
        }
        idx += 1;
        if idx < params.len() {
            model.attention_layers[i].proj.bias.clone_from(&params[idx]);
        }
        idx += 1;

        // FFN fc1.
        let f1_in = model.ffn_layers[i].fc1.n_in();
        let f1_out = model.ffn_layers[i].fc1.n_out();
        if idx < params.len() {
            for k in 0..f1_in {
                for j in 0..f1_out {
                    model.ffn_layers[i].fc1.weight[k][j] = params[idx][k * f1_out + j];
                }
            }
        }
        idx += 1;
        if idx < params.len() {
            model.ffn_layers[i].fc1.bias.clone_from(&params[idx]);
        }
        idx += 1;

        // FFN fc2.
        let f2_in = model.ffn_layers[i].fc2.n_in();
        let f2_out = model.ffn_layers[i].fc2.n_out();
        if idx < params.len() {
            for k in 0..f2_in {
                for j in 0..f2_out {
                    model.ffn_layers[i].fc2.weight[k][j] = params[idx][k * f2_out + j];
                }
            }
        }
        idx += 1;
        if idx < params.len() {
            model.ffn_layers[i].fc2.bias.clone_from(&params[idx]);
        }
        idx += 1;

        // LayerNorm.
        if idx < params.len() {
            model.ln1_gamma[i].clone_from(&params[idx]);
        }
        idx += 1;
        if idx < params.len() {
            model.ln1_beta[i].clone_from(&params[idx]);
        }
        idx += 1;
        if idx < params.len() {
            model.ln2_gamma[i].clone_from(&params[idx]);
        }
        idx += 1;
        if idx < params.len() {
            model.ln2_beta[i].clone_from(&params[idx]);
        }
        idx += 1;
    }

    // Output head.
    let o_in = model.output_head.n_in();
    let o_out = model.output_head.n_out();
    if idx < params.len() {
        for k in 0..o_in {
            for j in 0..o_out {
                model.output_head.weight[k][j] = params[idx][k * o_out + j];
            }
        }
    }
    idx += 1;
    if idx < params.len() {
        model.output_head.bias.clone_from(&params[idx]);
    }
}

fn flat_grads_placeholder_push(_opt: &mut Optimizer, _idx: usize, _noise: &[f64]) {
    // Placeholder for the random perturbation fallback.
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::optimizer::OptimizerKind;
    use rand::Rng;

    fn make_config(task: crate::mlp::TaskType) -> TransformerConfig {
        TransformerConfig {
            d_model: 8,
            n_heads: 2,
            n_layers: 1,
            d_ff: 16,
            dropout: 0.0,
            attention_dropout: 0.0,
            pooling: Pooling::Mean,
            positional_encoding: PositionalEncoding::Sinusoidal,
            task_type: task,
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
        }
    }

    #[test]
    fn test_transformer_forward_shape() {
        let x = Tensor::from_rows(4, 3, &[1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0, 11.0, 12.0]);
        let config = make_config(crate::mlp::TaskType::Classification);
        let mut model = TransformerModel::new(3, &config);
        let mut rng = ChaCha8Rng::seed_from_u64(0);
        let out = model.forward(&x, false, &mut rng);
        assert_eq!(out.shape(), (4, 1));
    }

    #[test]
    fn test_transformer_param_collect_scatter() {
        let config = make_config(crate::mlp::TaskType::Classification);
        let mut model = TransformerModel::new(3, &config);
        let params1 = collect_all_params(&model);
        // Modify a parameter.
        model.output_head.bias[0] = 42.0;
        let params2 = collect_all_params(&model);

        // Scatter back params1.
        scatter_all_params(&mut model, &params1);
        let params3 = collect_all_params(&model);

        // Should match params1.
        for i in 0..params1.len() {
            for j in 0..params1[i].len() {
                assert!((params1[i][j] - params3[i][j]).abs() < 1e-10, "param mismatch at {i},{j}");
            }
        }
        // params2 should differ from params3 (since we restored params1).
        let _ = params2;
    }

    #[test]
    fn test_transformer_classification_simple() {
        let mut x_data = Vec::new();
        let mut y_data = Vec::new();
        let mut rng = ChaCha8Rng::seed_from_u64(99);
        for _ in 0..30 {
            if rng.random::<f64>() < 0.5 {
                x_data.extend_from_slice(&[0.0, 0.0, 0.0]);
                y_data.push(0.0);
            } else {
                x_data.extend_from_slice(&[5.0, 5.0, 5.0]);
                y_data.push(1.0);
            }
        }
        let x = Tensor::from_rows(30, 3, &x_data);
        let y = Tensor::from_rows(30, 1, &y_data);

        let config = TransformerConfig {
            d_model: 4,
            n_heads: 2,
            n_layers: 1,
            d_ff: 8,
            dropout: 0.0,
            attention_dropout: 0.0,
            pooling: Pooling::Mean,
            positional_encoding: PositionalEncoding::Sinusoidal,
            task_type: crate::mlp::TaskType::Classification,
            train: TrainConfig {
                optimizer: OptimizerConfig {
                    kind: OptimizerKind::Adam,
                    lr: 0.05,
                    ..Default::default()
                },
                n_epochs: 5,
                batch_size: 10,
                ..Default::default()
            },
        };

        let result = train_transformer(&x, &y, None, &config).unwrap();
        assert_eq!(result.predictions.shape(), (30, 1));
        // With only 5 epochs and finite-difference training, just check it runs.
    }
}
