//! RNN / LSTM / GRU — recurrent neural networks for longitudinal/sequence data.
//!
//! Supports configurable cell types (vanilla RNN, LSTM, GRU), bidirectional
//! processing, and multiple pooling strategies (last / mean / max).
//!
//! Weight matrices are stored as flat vectors per (layer, direction) with
//! `wx_rows` / `wh_rows` tracking the matrix shape for indexing.

use rand::SeedableRng;
use rand::seq::SliceRandom;
use rand::Rng;
use rand_chacha::ChaCha8Rng;
use serde::{Deserialize, Serialize};

use crate::layers::{Activation, InitScheme, Linear};
use crate::losses::{binary_cross_entropy, mse, sigmoid_stable};
use crate::mlp::{clip_gradients, extract_rows, compute_val_metrics, EarlyStoppingConfig, EpochLog, TrainConfig};
use crate::optimizer::{Optimizer, OptimizerConfig};
use crate::scaler::StandardScaler;
use crate::tensor::Tensor;

/// RNN cell type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CellType {
    Rnn,
    Lstm,
    Gru,
}

/// Pooling strategy for sequence output.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SeqPooling {
    Last,
    Mean,
    Max,
}

/// Configuration for RNN/LSTM/GRU.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RnnConfig {
    pub cell_type: CellType,
    pub hidden_size: usize,
    pub n_layers: usize,
    pub bidirectional: bool,
    pub dropout: f64,
    pub pooling: SeqPooling,
    pub task_type: crate::mlp::TaskType,
    #[serde(flatten)]
    pub train: TrainConfig,
}

impl Default for RnnConfig {
    fn default() -> Self {
        Self {
            cell_type: CellType::Lstm,
            hidden_size: 64,
            n_layers: 1,
            bidirectional: false,
            dropout: 0.0,
            pooling: SeqPooling::Last,
            task_type: crate::mlp::TaskType::Classification,
            train: TrainConfig::default(),
        }
    }
}

/// Fitted RNN model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RnnModel {
    /// Input-to-hidden weights per (layer, direction), stored flat (row-major).
    /// Shape: rows = n_gates * hidden_size, cols = input_dim (varies per layer).
    pub wx: Vec<Vec<Vec<f64>>>,       // [layer * n_dirs][rows][cols]
    /// Hidden-to-hidden weights per (layer, direction).
    /// Shape: rows = n_gates * hidden_size, cols = hidden_size.
    pub wh: Vec<Vec<Vec<f64>>>,       // [layer * n_dirs][rows][cols]
    /// Bias per (layer, direction): length n_gates * hidden_size.
    pub bias: Vec<Vec<f64>>,          // [layer * n_dirs]
    /// Output head.
    pub output_head: Linear,
    pub scaler: Option<StandardScaler>,
    pub config: RnnConfig,
    pub n_seq_features: usize,
    pub n_static_features: usize,
    pub seq_length: usize,
    pub training_log: Vec<EpochLog>,
}

pub struct RnnTrainOutput {
    pub model: RnnModel,
    pub predictions: Tensor,
    pub training_log: Vec<EpochLog>,
}

impl RnnModel {
    pub fn new(n_seq_features: usize, n_static_features: usize, seq_length: usize, config: &RnnConfig) -> Self {
        let mut rng = ChaCha8Rng::seed_from_u64(config.train.seed);
        let h = config.hidden_size;
        let n_dirs = if config.bidirectional { 2 } else { 1 };
        let n_gates = match config.cell_type {
            CellType::Rnn => 1,
            CellType::Lstm => 4,
            CellType::Gru => 3,
        };
        let input_size = n_seq_features + n_static_features;

        let mut wx = Vec::with_capacity(config.n_layers * n_dirs);
        let mut wh = Vec::with_capacity(config.n_layers * n_dirs);
        let mut bias = Vec::with_capacity(config.n_layers * n_dirs);

        for layer in 0..config.n_layers {
            let prev_h = if layer == 0 { input_size } else { h * n_dirs };
            for _d in 0..n_dirs {
                // Wx: (n_gates*h, prev_h).
                let scale = (2.0 / prev_h as f64).sqrt();
                let w: Vec<Vec<f64>> = (0..n_gates * h)
                    .map(|_| (0..prev_h).map(|_| rng.random::<f64>() * 2.0 * scale - scale).collect())
                    .collect();
                wx.push(w);

                // Wh: (n_gates*h, h).
                let scale_h = (2.0 / h as f64).sqrt();
                let w2: Vec<Vec<f64>> = (0..n_gates * h)
                    .map(|_| (0..h).map(|_| rng.random::<f64>() * 2.0 * scale_h - scale_h).collect())
                    .collect();
                wh.push(w2);

                bias.push((0..n_gates * h).map(|_| rng.random::<f64>() * 0.02 - 0.01).collect());
            }
        }

        let out_input = h * n_dirs;
        let output_head = Linear::new(out_input, 1, &mut rng, InitScheme::Xavier);

        Self {
            wx, wh, bias, output_head, scaler: None,
            config: config.clone(),
            n_seq_features, n_static_features, seq_length,
            training_log: Vec::new(),
        }
    }

    /// RNN cell step for one (layer_dir_idx, time step).
    /// `wx` is `[n_gates*h][input_dim]`, `wh` is `[n_gates*h][h]`.
    fn cell_step(&self, ld_idx: usize, x_t: &[f64], h_prev: &[f64]) -> Vec<f64> {
        let h = self.config.hidden_size;
        let n_gates = match self.config.cell_type {
            CellType::Rnn => 1,
            CellType::Lstm => 4,
            CellType::Gru => 3,
        };

        // Compute pre-activations z[i] = sum_j wx[i][j]*x_t[j] + sum_j wh[i][j]*h_prev[j] + bias[i].
        let mut z = vec![0.0; n_gates * h];
        let wx_ld = &self.wx[ld_idx];
        let wh_ld = &self.wh[ld_idx];
        let b_ld = &self.bias[ld_idx];

        for i in 0..(n_gates * h) {
            let mut sum = b_ld[i];
            for (j, &xv) in x_t.iter().enumerate() {
                sum += wx_ld[i][j] * xv;
            }
            for (j, &hv) in h_prev.iter().enumerate() {
                sum += wh_ld[i][j] * hv;
            }
            z[i] = sum;
        }

        match self.config.cell_type {
            CellType::Rnn => {
                (0..h).map(|i| z[i].tanh()).collect()
            }
            CellType::Lstm => {
                let i_gate: Vec<f64> = (0..h).map(|i| sigmoid_stable(z[i])).collect();
                let f_gate: Vec<f64> = (0..h).map(|i| sigmoid_stable(z[h + i])).collect();
                let g_gate: Vec<f64> = (0..h).map(|i| z[2*h + i].tanh()).collect();
                let o_gate: Vec<f64> = (0..h).map(|i| sigmoid_stable(z[3*h + i])).collect();

                let c_new: Vec<f64> = (0..h)
                    .map(|i| f_gate[i] * h_prev.get(i).copied().unwrap_or(0.0) + i_gate[i] * g_gate[i])
                    .collect();
                (0..h).map(|i| o_gate[i] * c_new[i].tanh()).collect()
            }
            CellType::Gru => {
                let r_gate: Vec<f64> = (0..h).map(|i| sigmoid_stable(z[i])).collect();
                let u_gate: Vec<f64> = (0..h).map(|i| sigmoid_stable(z[h + i])).collect();

                // Candidate with reset gate applied to hidden.
                let mut h_new = vec![0.0; h];
                for i in 0..h {
                    // z[2*h + i] already includes wh @ h_prev. Adjust for reset.
                    let old_contrib: f64 = (0..h).map(|j| wh_ld[2*h + i][j] * h_prev[j]).sum();
                    let new_contrib: f64 = (0..h).map(|j| wh_ld[2*h + i][j] * r_gate[j] * h_prev[j]).sum();
                    let adjusted = z[2*h + i] - old_contrib + new_contrib;
                    h_new[i] = (1.0 - u_gate[i]) * adjusted.tanh() + u_gate[i] * h_prev.get(i).copied().unwrap_or(0.0);
                }
                h_new
            }
        }
    }

    /// Forward pass.
    /// Input: (batch, n_seq_features*seq_length + n_static_features).
    pub fn forward(&mut self, x: &Tensor, _training: bool, _rng: &mut ChaCha8Rng) -> Tensor {
        let batch = x.nrows();
        let h = self.config.hidden_size;
        let n_dirs = if self.config.bidirectional { 2 } else { 1 };
        let seq_len = self.seq_length;
        let n_seq = self.n_seq_features;
        let n_static = self.n_static_features;

        let mut pooled: Vec<Vec<f64>> = Vec::with_capacity(batch);

        for i in 0..batch {
            let row: Vec<f64> = x.row(i);

            let static_part: Vec<f64> = if n_static > 0 {
                row[n_seq * seq_len..].to_vec()
            } else {
                Vec::new()
            };

            // Process layers.
            let mut layer_input_dim = n_seq + n_static;
            let mut layer_input: Vec<Vec<f64>> = Vec::new(); // per-step input for current layer

            for layer in 0..self.config.n_layers {
                let n_units_this_layer = if layer == 0 { layer_input_dim } else { h * n_dirs };

                // Build per-step inputs for this layer.
                let step_inputs: Vec<Vec<f64>> = if layer == 0 {
                    (0..seq_len).map(|step| {
                        let mut si = Vec::with_capacity(n_seq + n_static);
                        for f in 0..n_seq {
                            si.push(row[f * seq_len + step]);
                        }
                        si.extend_from_slice(&static_part);
                        si
                    }).collect()
                } else {
                    layer_input.clone()
                };

                let mut all_dir_outputs: Vec<Vec<Vec<f64>>> = Vec::new();

                for dir in 0..n_dirs {
                    let ld_idx = layer * n_dirs + dir;
                    let mut hidden = vec![0.0; h];
                    let mut step_outs: Vec<Vec<f64>> = Vec::with_capacity(seq_len);

                    let steps: Vec<usize> = if dir == 0 {
                        (0..seq_len).collect()
                    } else {
                        (0..seq_len).rev().collect()
                    };

                    for &step in &steps {
                        hidden = self.cell_step(ld_idx, &step_inputs[step], &hidden);
                        step_outs.push(hidden.clone());
                    }

                    if dir == 1 { step_outs.reverse(); }
                    all_dir_outputs.push(step_outs);
                }

                // Concatenate directions per step.
                let concat: Vec<Vec<f64>> = (0..seq_len).map(|step| {
                    let mut v = Vec::with_capacity(h * n_dirs);
                    for dir in 0..n_dirs {
                        v.extend_from_slice(&all_dir_outputs[dir][step]);
                    }
                    v
                }).collect();

                layer_input = concat;
                layer_input_dim = n_units_this_layer;
            }

            // Pool last layer.
            let last = &layer_input;
            let p = match self.config.pooling {
                SeqPooling::Last => last.last().unwrap().clone(),
                SeqPooling::Mean => {
                    let n = last.len();
                    (0..last[0].len()).map(|j| last.iter().map(|s| s[j]).sum::<f64>() / n as f64).collect()
                }
                SeqPooling::Max => {
                    let dim = last[0].len();
                    (0..dim).map(|j| last.iter().map(|s| s[j]).fold(f64::NEG_INFINITY, f64::max)).collect()
                }
            };
            pooled.push(p);
        }

        let flat: Vec<f64> = pooled.iter().flatten().cloned().collect();
        let pooled_tensor = Tensor::from_rows(batch, h * n_dirs, &flat);
        self.output_head.forward(&pooled_tensor)
    }

    pub fn n_params(&self) -> usize {
        let wx_count: usize = self.wx.iter().map(|m| m.len() * m.first().map(|r| r.len()).unwrap_or(0)).sum();
        let wh_count: usize = self.wh.iter().map(|m| m.len() * m.first().map(|r| r.len()).unwrap_or(0)).sum();
        wx_count + wh_count + self.bias.iter().map(|b| b.len()).sum::<usize>() + self.output_head.n_params()
    }
}

/// Train an RNN/LSTM/GRU model.
pub fn train_rnn(
    x: &Tensor,
    y: &Tensor,
    val: Option<(&Tensor, &Tensor)>,
    config: &RnnConfig,
    n_seq_features: usize,
    n_static_features: usize,
    seq_length: usize,
) -> Result<RnnTrainOutput, String> {
    let (nrows, _ncols) = x.shape();
    if nrows == 0 {
        return Err("empty training data".into());
    }

    let scaler = if config.train.standardize {
        Some(StandardScaler::fit(x))
    } else {
        None
    };
    let x_train = scaler.as_ref().map(|s| s.transform(x)).unwrap_or_else(|| x.clone());

    let mut model = RnnModel::new(n_seq_features, n_static_features, seq_length, config);
    let all_param_sizes = collect_rnn_param_sizes(&model);
    let mut opt = Optimizer::new(config.train.optimizer.clone(), &all_param_sizes);
    let mut rng = ChaCha8Rng::seed_from_u64(config.train.seed);
    let batch_size = config.train.batch_size.min(nrows);

    let mut training_log = Vec::new();
    let mut best_metric = f64::INFINITY;
    let mut bad_epochs = 0usize;

    for epoch in 0..config.train.n_epochs {
        let mut indices: Vec<usize> = (0..nrows).collect();
        indices.shuffle(&mut rng);

        let mut epoch_loss = 0.0;
        let mut n_batches = 0;

        for chunk in indices.chunks(batch_size) {
            let x_batch = extract_rows(&x_train, chunk);
            let y_batch = extract_rows(y, chunk);

            let output = model.forward(&x_batch, true, &mut rng);
            let (loss, _) = match config.task_type {
                crate::mlp::TaskType::Classification => {
                    let t: Vec<f64> = (0..y_batch.nrows()).map(|i| y_batch.at(i, 0)).collect();
                    binary_cross_entropy(&output, &t)
                }
                crate::mlp::TaskType::Regression => mse(&output, &y_batch),
            };
            epoch_loss += loss;
            n_batches += 1;

            // Finite-difference gradient for small models.
            let current_params = collect_rnn_params(&model);
            let n_total: usize = current_params.iter().map(|p| p.len()).sum();

            if n_total <= 3000 {
                let eps_fd = 1e-5;
                let mut flat_grads: Vec<Vec<f64>> = Vec::with_capacity(current_params.len());

                for (pi, param_vec) in current_params.iter().enumerate() {
                    let mut grad_vec = vec![0.0; param_vec.len()];
                    for ii in 0..param_vec.len() {
                        let mut perturbed = current_params.clone();
                        perturbed[pi][ii] += eps_fd;
                        scatter_rnn_params(&mut model, &perturbed);
                        let pred_plus = model.forward(&x_batch, false, &mut rng);
                        let (loss_plus, _) = match config.task_type {
                            crate::mlp::TaskType::Classification => {
                                let t: Vec<f64> = (0..y_batch.nrows()).map(|j| y_batch.at(j, 0)).collect();
                                binary_cross_entropy(&pred_plus, &t)
                            }
                            crate::mlp::TaskType::Regression => mse(&pred_plus, &y_batch),
                        };

                        perturbed[pi][ii] -= 2.0 * eps_fd;
                        scatter_rnn_params(&mut model, &perturbed);
                        let pred_minus = model.forward(&x_batch, false, &mut rng);
                        let (loss_minus, _) = match config.task_type {
                            crate::mlp::TaskType::Classification => {
                                let t: Vec<f64> = (0..y_batch.nrows()).map(|j| y_batch.at(j, 0)).collect();
                                binary_cross_entropy(&pred_minus, &t)
                            }
                            crate::mlp::TaskType::Regression => mse(&pred_minus, &y_batch),
                        };
                        grad_vec[ii] = (loss_plus - loss_minus) / (2.0 * eps_fd);
                    }
                    flat_grads.push(grad_vec);
                }

                scatter_rnn_params(&mut model, &current_params);
                if let Some(max_norm) = config.train.gradient_clip_norm {
                    clip_gradients(&mut flat_grads, max_norm);
                }
                opt.read_params(&current_params);
                opt.step(&flat_grads);
                scatter_rnn_params(&mut model, &opt.write_params());
            }
        }

        let train_loss = epoch_loss / n_batches as f64;
        let (val_loss, val_metric) = if let Some((xv, yv)) = val {
            let preds = model.forward(xv, false, &mut rng);
            let (vl, vm) = compute_val_metrics(&preds, yv, config.task_type);
            (Some(vl), Some(vm))
        } else {
            (None, None)
        };

        training_log.push(EpochLog {
            epoch, train_loss, val_loss, val_metric,
            lr: config.train.optimizer.lr,
        });

        if let Some(ref es) = config.train.early_stopping {
            let mv = val_loss.unwrap_or(train_loss);
            if mv < best_metric {
                best_metric = mv;
                bad_epochs = 0;
            } else {
                bad_epochs += 1;
            }
            if bad_epochs >= es.patience { break; }
        }
    }

    model.scaler = scaler;
    model.training_log = training_log.clone();
    let predictions = model.forward(&x_train, false, &mut rng);

    Ok(RnnTrainOutput { model, predictions, training_log })
}

pub fn predict_rnn(model: &mut RnnModel, x: &Tensor) -> Tensor {
    let x_scaled = model.scaler.as_ref().map(|s| s.transform(x)).unwrap_or_else(|| x.clone());
    let mut rng = ChaCha8Rng::seed_from_u64(model.config.train.seed);
    let logits = model.forward(&x_scaled, false, &mut rng);

    match model.config.task_type {
        crate::mlp::TaskType::Classification => {
            let batch = logits.nrows();
            let mut probs = Tensor::zeros(batch, 1);
            for i in 0..batch {
                probs.set(i, 0, sigmoid_stable(logits.at(i, 0)));
            }
            probs
        }
        crate::mlp::TaskType::Regression => logits,
    }
}

fn collect_rnn_param_sizes(model: &RnnModel) -> Vec<usize> {
    let mut sizes = Vec::new();
    for m in &model.wx {
        sizes.push(m.len() * m.first().map(|r| r.len()).unwrap_or(0));
    }
    for m in &model.wh {
        sizes.push(m.len() * m.first().map(|r| r.len()).unwrap_or(0));
    }
    for b in &model.bias {
        sizes.push(b.len());
    }
    sizes.push(model.output_head.n_in() * model.output_head.n_out());
    sizes.push(model.output_head.n_out());
    sizes
}

fn collect_rnn_params(model: &RnnModel) -> Vec<Vec<f64>> {
    let mut params = Vec::new();
    for m in &model.wx {
        let mut flat = Vec::new();
        for row in m { flat.extend_from_slice(row); }
        params.push(flat);
    }
    for m in &model.wh {
        let mut flat = Vec::new();
        for row in m { flat.extend_from_slice(row); }
        params.push(flat);
    }
    for b in &model.bias {
        params.push(b.clone());
    }
    let mut oh_w = Vec::new();
    for row in &model.output_head.weight { oh_w.extend_from_slice(row); }
    params.push(oh_w);
    params.push(model.output_head.bias.clone());
    params
}

fn scatter_rnn_params(model: &mut RnnModel, params: &[Vec<f64>]) {
    let mut idx = 0;
    for m in &mut model.wx {
        let rows = m.len();
        let cols = m.first().map(|r| r.len()).unwrap_or(0);
        if idx < params.len() {
            let p = &params[idx];
            for i in 0..rows {
                for j in 0..cols {
                    m[i][j] = p[i * cols + j];
                }
            }
        }
        idx += 1;
    }
    for m in &mut model.wh {
        let rows = m.len();
        let cols = m.first().map(|r| r.len()).unwrap_or(0);
        if idx < params.len() {
            let p = &params[idx];
            for i in 0..rows {
                for j in 0..cols {
                    m[i][j] = p[i * cols + j];
                }
            }
        }
        idx += 1;
    }
    for b in &mut model.bias {
        if idx < params.len() { b.clone_from(&params[idx]); }
        idx += 1;
    }
    let ni = model.output_head.n_in();
    let no = model.output_head.n_out();
    if idx < params.len() {
        for k in 0..ni { for j in 0..no { model.output_head.weight[k][j] = params[idx][k * no + j]; } }
    }
    idx += 1;
    if idx < params.len() { model.output_head.bias.clone_from(&params[idx]); }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::optimizer::OptimizerKind;

    fn make_config(cell: CellType) -> RnnConfig {
        RnnConfig {
            cell_type: cell,
            hidden_size: 4,
            n_layers: 1,
            bidirectional: false,
            dropout: 0.0,
            pooling: SeqPooling::Last,
            task_type: crate::mlp::TaskType::Classification,
            train: TrainConfig {
                optimizer: OptimizerConfig {
                    kind: OptimizerKind::Adam, lr: 0.01, ..Default::default()
                },
                n_epochs: 10, batch_size: 8, ..Default::default()
            },
        }
    }

    #[test]
    fn test_rnn_forward_shape() {
        let config = make_config(CellType::Rnn);
        let mut model = RnnModel::new(2, 0, 3, &config);
        let x = Tensor::from_rows(4, 6, &[1.0; 24]);
        let out = model.forward(&x, false, &mut ChaCha8Rng::seed_from_u64(0));
        assert_eq!(out.shape(), (4, 1));
    }

    #[test]
    fn test_lstm_forward_shape() {
        let config = make_config(CellType::Lstm);
        let mut model = RnnModel::new(2, 0, 3, &config);
        let x = Tensor::from_rows(4, 6, &[1.0; 24]);
        let out = model.forward(&x, false, &mut ChaCha8Rng::seed_from_u64(0));
        assert_eq!(out.shape(), (4, 1));
    }

    #[test]
    fn test_gru_forward_shape() {
        let config = make_config(CellType::Gru);
        let mut model = RnnModel::new(2, 0, 3, &config);
        let x = Tensor::from_rows(4, 6, &[1.0; 24]);
        let out = model.forward(&x, false, &mut ChaCha8Rng::seed_from_u64(0));
        assert_eq!(out.shape(), (4, 1));
    }

    #[test]
    fn test_rnn_bidirectional() {
        let mut config = make_config(CellType::Lstm);
        config.bidirectional = true;
        let mut model = RnnModel::new(2, 0, 3, &config);
        let x = Tensor::from_rows(4, 6, &[1.0; 24]);
        let out = model.forward(&x, false, &mut ChaCha8Rng::seed_from_u64(0));
        assert_eq!(out.shape(), (4, 1));
    }

    #[test]
    fn test_rnn_serialization() {
        let config = make_config(CellType::Lstm);
        let model = RnnModel::new(2, 0, 3, &config);
        let json = serde_json::to_string(&model).unwrap();
        let restored: RnnModel = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.n_seq_features, 2);
        assert_eq!(restored.seq_length, 3);
    }
}
