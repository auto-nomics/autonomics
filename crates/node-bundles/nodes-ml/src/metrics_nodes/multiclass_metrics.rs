use super::*;

use arrow_array::UInt32Array;

// ═══════════════════════════════════════════════════════════════════════
// MulticlassMetrics — probability-based k-class evaluation
// ═══════════════════════════════════════════════════════════════════════
//
// Port 0 expects a prediction table: an integer `label_column` in 0..k plus
// one probability column per class (`prob_columns` order = class id), as
// emitted by the multiclass fit/predict nodes.  Outputs:
//   p0 summary — single row of point estimates; when `cluster_column` is
//      given, each of accuracy / balanced_accuracy / macro_auc / brier /
//      log_loss also gets a cluster-bootstrap percentile CI (whole groups
//      are resampled with replacement, so repeated rows of one patient
//      never split across the resample);
//   p1 per-class — support, AUC, precision, recall, F1, calibration
//      slope/intercept;
//   p2 confusion matrix — long form (actual, predicted, count);
//   p3 calibration bins — long form over classes × non-empty bins.
//
// Bootstrap CIs use one seed for all metrics, so every replicate set is
// drawn from the same resamples (paired CIs).

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct MulticlassMetricsSpec {
    /// Column with ground-truth integer class labels 0..k-1, ordered to
    /// match `prob_columns`.
    pub label_column: String,
    /// Ordered probability columns, one per class: `prob_columns[i]` holds
    /// class i's probability.  Values must lie in [0, 1].
    pub prob_columns: Vec<String>,
    /// Optional cluster column (e.g. patient_id).  When given, every
    /// summary metric gets a cluster-bootstrap percentile CI.
    #[serde(default)]
    pub cluster_column: Option<String>,
    /// Bootstrap replicates per CI.
    #[serde(default = "d_n_boot")]
    pub n_boot: usize,
    /// Bootstrap seed (ChaCha8).
    #[serde(default)]
    pub seed: u64,
    /// CI coverage level.
    #[serde(default = "d_ci_level")]
    pub ci_level: f64,
    /// Equal-width calibration bins per class.
    #[serde(default = "d_calibration_bins")]
    pub calibration_bins: usize,
}
fn d_n_boot() -> usize {
    2000
}
fn d_ci_level() -> f64 {
    0.95
}
fn d_calibration_bins() -> usize {
    10
}

pub struct MulticlassMetricsFactory;
impl NodeFactory for MulticlassMetricsFactory {
    fn kind(&self) -> &'static str {
        "ml_multiclass_metrics"
    }
    fn desc(&self) -> &'static str {
        "Probability-based multiclass metrics with optional cluster-bootstrap CIs."
    }
    fn doc(&self) -> &'static str {
        "MulticlassMetrics: reads an integer label column plus one probability \
        column per class and emits four ports — p0 single-row summary (accuracy, \
        balanced accuracy, macro OvR AUC, macro precision/recall/F1, Brier, log \
        loss; with cluster-bootstrap percentile CIs when cluster_column is \
        given), p1 per-class metrics incl. calibration slope/intercept, p2 \
        confusion matrix (long form), p3 calibration bins (long form). \
        Predicted class = argmax of the probability row, first max wins ties."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(MulticlassMetricsSpec)
    }
    fn ports(&self) -> NodePorts {
        NodePorts::new()
            .add_input_port(None)
            .add_output_port(None) // p0 summary
            .add_output_port(None) // p1 per-class
            .add_output_port(None) // p2 confusion
            .add_output_port(None) // p3 calibration
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _ctx: NodeCtx,
    ) -> Result<Box<dyn DagNode>, dag_core::registry::error::Error> {
        let s: MulticlassMetricsSpec = serde_json::from_value(spec)?;
        Ok(Box::new(MulticlassMetricsNode {
            label_column: s.label_column,
            prob_columns: s.prob_columns,
            cluster_column: s.cluster_column,
            n_boot: s.n_boot,
            seed: s.seed,
            ci_level: s.ci_level,
            calibration_bins: s.calibration_bins,
            meta: self.ports(),
        }))
    }
}

#[derive(Clone)]
struct MulticlassMetricsNode {
    label_column: String,
    prob_columns: Vec<String>,
    cluster_column: Option<String>,
    n_boot: usize,
    seed: u64,
    ci_level: f64,
    calibration_bins: usize,
    meta: NodePorts,
}

#[async_trait]
impl DagNode for MulticlassMetricsNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }
    fn kind(&self) -> &'static str {
        "ml_multiclass_metrics"
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    async fn execute(
        &mut self,
        ctx: &NodeCtx,
        inputs: &[NodeInput],
        _r: &dag_core::dag::node_event::NodeReporter,
    ) -> Result<PortOutputs, DagError> {
        let ne = |msg: String| DagError::NodeError {
            node_type: "ml_multiclass_metrics".into(),
            msg,
        };

        let batches = collect_batches(inputs).await?;
        let k = self.prob_columns.len();
        if k < 2 {
            return Err(ne(format!("need >= 2 prob_columns, got {k}")));
        }

        let y_raw = common::extract_numeric_column(&batches, &self.label_column)
            .map_err(|e| ne(e.to_string()))?;
        let y = labels_from_f64(&y_raw, k).map_err(ne)?;
        let n = y.len();
        if n == 0 {
            return Err(ne("no input rows".into()));
        }

        let mut cols: Vec<Vec<f64>> = Vec::with_capacity(k);
        for name in &self.prob_columns {
            let c =
                common::extract_numeric_column(&batches, name).map_err(|e| ne(e.to_string()))?;
            if c.len() != n {
                return Err(ne(format!(
                    "prob column '{name}' has {} rows, label has {n}",
                    c.len()
                )));
            }
            for (i, &v) in c.iter().enumerate() {
                if !v.is_finite() || !(0.0..=1.0).contains(&v) {
                    return Err(ne(format!(
                        "prob column '{name}' row {i}: value {v} not in [0,1]"
                    )));
                }
            }
            cols.push(c);
        }
        let rows: Vec<Vec<f64>> = (0..n)
            .map(|i| cols.iter().map(|c| c[i]).collect())
            .collect();

        // ── point estimates ──────────────────────────────────────────────
        let all: Vec<usize> = (0..n).collect();
        let cm = confusion_idx(&all, &y, &rows, k);
        let y_f: Vec<f64> = y.iter().map(|&v| v as f64).collect();
        let y_pred: Vec<usize> = rows.iter().map(|r| argmax(r)).collect();
        let y_pred_f: Vec<f64> = y_pred.iter().map(|&v| v as f64).collect();

        let accuracy = ml::metrics::accuracy(&y_f, &y_pred_f).map_err(|e| ne(e.to_string()))?;
        let balanced = ml::metrics::balanced_accuracy(&cm, k).map_err(|e| ne(e.to_string()))?;
        let aucs = ml::metrics::ovr_auc(&y, &rows, k).map_err(|e| ne(e.to_string()))?;
        let macro_auc = ml::metrics::macro_ovr_auc(&y, &rows, k).map_err(|e| ne(e.to_string()))?;
        let brier = ml::metrics::multiclass_brier(&y, &rows, k).map_err(|e| ne(e.to_string()))?;
        let log_loss =
            ml::metrics::multiclass_log_loss(&y, &rows, k).map_err(|e| ne(e.to_string()))?;
        let (m_p, m_r, m_f) = ml::metrics::macro_precision_recall_f1(&y_f, &y_pred_f, k)
            .map_err(|e| ne(e.to_string()))?;
        let prf = per_class_prf(&cm, k);

        let calib = (0..k)
            .map(|c| {
                let y_bin: Vec<bool> = y.iter().map(|&v| v == c).collect();
                let p: Vec<f64> = rows.iter().map(|r| r[c]).collect();
                ml::metrics::class_calibration(&y_bin, &p, self.calibration_bins)
            })
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| ne(e.to_string()))?;

        // ── cluster-bootstrap CIs (optional) ─────────────────────────────
        let clusters: Option<Vec<u64>> = match &self.cluster_column {
            Some(col) => {
                let keys = common::group_keys(&batches, col).map_err(ne)?;
                if keys.len() != n {
                    return Err(ne(format!(
                        "cluster column '{col}' has {} rows, label has {n}",
                        keys.len()
                    )));
                }
                Some(
                    common::dense_group_ids(&keys)
                        .into_iter()
                        .map(|g| g as u64)
                        .collect(),
                )
            }
            None => None,
        };

        // ── p0 summary ───────────────────────────────────────────────────
        let mut fields: Vec<Arc<Field>> = vec![
            Arc::new(Field::new("n_samples", DataType::Float64, false)),
            Arc::new(Field::new("n_classes", DataType::Float64, false)),
            Arc::new(Field::new("accuracy", DataType::Float64, false)),
            Arc::new(Field::new("balanced_accuracy", DataType::Float64, false)),
            Arc::new(Field::new("macro_auc", DataType::Float64, false)),
            Arc::new(Field::new("macro_precision", DataType::Float64, false)),
            Arc::new(Field::new("macro_recall", DataType::Float64, false)),
            Arc::new(Field::new("macro_f1", DataType::Float64, false)),
            Arc::new(Field::new("brier", DataType::Float64, false)),
            Arc::new(Field::new("log_loss", DataType::Float64, false)),
        ];
        let mut vals: Vec<f64> = vec![
            n as f64, k as f64, accuracy, balanced, macro_auc, m_p, m_r, m_f, brier, log_loss,
        ];
        if let Some(cl) = &clusters {
            if !(0.0..1.0).contains(&self.ci_level) {
                return Err(ne(format!(
                    "ci_level must be in (0,1), got {}",
                    self.ci_level
                )));
            }
            let n_clusters = cl.iter().collect::<std::collections::BTreeSet<_>>().len();
            fields.push(Arc::new(Field::new("n_clusters", DataType::Float64, false)));
            vals.push(n_clusters as f64);

            let boot = |metric: IdxMetric| -> Result<(f64, f64), DagError> {
                let reps = ml::metrics::cluster_bootstrap(
                    |idx: &[usize]| metric(idx, &y, &rows, k).unwrap_or(f64::NAN),
                    cl,
                    self.n_boot,
                    self.seed,
                )
                .map_err(|e| ne(e.to_string()))?;
                let mut valid: Vec<f64> = reps.into_iter().filter(|v| !v.is_nan()).collect();
                if valid.len() < 2 {
                    return Ok((f64::NAN, f64::NAN));
                }
                valid.sort_by(|a, b| a.partial_cmp(b).unwrap());
                let alpha = (1.0 - self.ci_level) / 2.0;
                Ok((percentile(&valid, alpha), percentile(&valid, 1.0 - alpha)))
            };
            for (name, metric) in [
                ("accuracy", idx_accuracy as IdxMetric),
                ("balanced_accuracy", idx_balanced_accuracy),
                ("macro_auc", idx_macro_auc),
                ("brier", idx_brier),
                ("log_loss", idx_log_loss),
            ] {
                let (lo, hi) = boot(metric)?;
                fields.push(Arc::new(Field::new(
                    format!("{name}_lo"),
                    DataType::Float64,
                    false,
                )));
                fields.push(Arc::new(Field::new(
                    format!("{name}_hi"),
                    DataType::Float64,
                    false,
                )));
                vals.push(lo);
                vals.push(hi);
            }
        }
        let p0 = RecordBatch::try_new(
            Arc::new(Schema::new(fields)),
            vec![Arc::new(Float64Array::from(vals))],
        )
        .map_err(|e| ne(format!("build summary batch: {e}")))?;

        // ── p1 per-class ─────────────────────────────────────────────────
        let class_col: Vec<u32> = (0..k as u32).collect();
        let support: Vec<u32> = (0..k)
            .map(|c| (0..k).map(|j| cm[c * k + j]).sum::<usize>() as u32)
            .collect();
        let p1 = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Arc::new(Field::new("class", DataType::UInt32, false)),
                Arc::new(Field::new("support", DataType::UInt32, false)),
                Arc::new(Field::new("auc", DataType::Float64, false)),
                Arc::new(Field::new("precision", DataType::Float64, false)),
                Arc::new(Field::new("recall", DataType::Float64, false)),
                Arc::new(Field::new("f1", DataType::Float64, false)),
                Arc::new(Field::new("calib_slope", DataType::Float64, false)),
                Arc::new(Field::new("calib_intercept", DataType::Float64, false)),
            ])),
            vec![
                Arc::new(UInt32Array::from(class_col)),
                Arc::new(UInt32Array::from(support)),
                Arc::new(Float64Array::from(aucs)),
                Arc::new(Float64Array::from(
                    prf.iter().map(|p| p.0).collect::<Vec<_>>(),
                )),
                Arc::new(Float64Array::from(
                    prf.iter().map(|p| p.1).collect::<Vec<_>>(),
                )),
                Arc::new(Float64Array::from(
                    prf.iter().map(|p| p.2).collect::<Vec<_>>(),
                )),
                Arc::new(Float64Array::from(
                    calib.iter().map(|c| c.slope).collect::<Vec<_>>(),
                )),
                Arc::new(Float64Array::from(
                    calib.iter().map(|c| c.intercept).collect::<Vec<_>>(),
                )),
            ],
        )
        .map_err(|e| ne(format!("build per-class batch: {e}")))?;

        // ── p2 confusion (long form) ─────────────────────────────────────
        let mut actual: Vec<u32> = Vec::with_capacity(k * k);
        let mut predicted: Vec<u32> = Vec::with_capacity(k * k);
        let mut count: Vec<u32> = Vec::with_capacity(k * k);
        for a in 0..k {
            for p in 0..k {
                actual.push(a as u32);
                predicted.push(p as u32);
                count.push(cm[a * k + p] as u32);
            }
        }
        let p2 = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Arc::new(Field::new("actual", DataType::UInt32, false)),
                Arc::new(Field::new("predicted", DataType::UInt32, false)),
                Arc::new(Field::new("count", DataType::UInt32, false)),
            ])),
            vec![
                Arc::new(UInt32Array::from(actual)),
                Arc::new(UInt32Array::from(predicted)),
                Arc::new(UInt32Array::from(count)),
            ],
        )
        .map_err(|e| ne(format!("build confusion batch: {e}")))?;

        // ── p3 calibration bins (long form) ──────────────────────────────
        let mut cal_class: Vec<u32> = Vec::new();
        let mut cal_bin: Vec<u32> = Vec::new();
        let mut cal_n: Vec<u32> = Vec::new();
        let mut cal_mean: Vec<f64> = Vec::new();
        let mut cal_rate: Vec<f64> = Vec::new();
        for (c, curve) in calib.iter().enumerate() {
            for b in &curve.bins {
                cal_class.push(c as u32);
                cal_bin.push(b.bin_index as u32);
                cal_n.push(b.n as u32);
                cal_mean.push(b.mean_pred);
                cal_rate.push(b.obs_rate);
            }
        }
        let p3 = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Arc::new(Field::new("class", DataType::UInt32, false)),
                Arc::new(Field::new("bin_index", DataType::UInt32, false)),
                Arc::new(Field::new("n", DataType::UInt32, false)),
                Arc::new(Field::new("mean_pred", DataType::Float64, false)),
                Arc::new(Field::new("obs_rate", DataType::Float64, false)),
            ])),
            vec![
                Arc::new(UInt32Array::from(cal_class)),
                Arc::new(UInt32Array::from(cal_bin)),
                Arc::new(UInt32Array::from(cal_n)),
                Arc::new(Float64Array::from(cal_mean)),
                Arc::new(Float64Array::from(cal_rate)),
            ],
        )
        .map_err(|e| ne(format!("build calibration batch: {e}")))?;

        let sess = ctx.session();
        let mut res = PortOutputs::new();
        res.insert(
            0,
            sess.read_batch(p0)
                .map_err(|e| ne(format!("read_batch(0): {e}")))?,
        );
        res.insert(
            1,
            sess.read_batch(p1)
                .map_err(|e| ne(format!("read_batch(1): {e}")))?,
        );
        res.insert(
            2,
            sess.read_batch(p2)
                .map_err(|e| ne(format!("read_batch(2): {e}")))?,
        );
        res.insert(
            3,
            sess.read_batch(p3)
                .map_err(|e| ne(format!("read_batch(3): {e}")))?,
        );
        Ok(res)
    }
}

// ── pure helpers ─────────────────────────────────────────────────────────

/// Validate a numeric label column: finite integers in 0..k.
fn labels_from_f64(y: &[f64], k: usize) -> Result<Vec<usize>, String> {
    y.iter()
        .map(|&v| {
            if !v.is_finite() || v < 0.0 || v.fract() != 0.0 || v as usize >= k {
                Err(format!("label {v} is not an integer in 0..{k}"))
            } else {
                Ok(v as usize)
            }
        })
        .collect()
}

/// Index of the maximum value; first max wins ties (matches R `which.max`).
fn argmax(row: &[f64]) -> usize {
    let mut best = 0;
    let mut best_v = f64::NEG_INFINITY;
    for (j, &v) in row.iter().enumerate() {
        if v > best_v {
            best_v = v;
            best = j;
        }
    }
    best
}

/// Row-major confusion matrix over an index selection (replicates included).
fn confusion_idx(idx: &[usize], y: &[usize], rows: &[Vec<f64>], k: usize) -> Vec<usize> {
    let mut cm = vec![0usize; k * k];
    for &i in idx {
        cm[y[i] * k + argmax(&rows[i])] += 1;
    }
    cm
}

/// Per-class (precision, recall, F1) from a row-major confusion matrix.
fn per_class_prf(cm: &[usize], k: usize) -> Vec<(f64, f64, f64)> {
    (0..k)
        .map(|c| {
            let tp = cm[c * k + c];
            let fp: usize = (0..k).filter(|&i| i != c).map(|i| cm[i * k + c]).sum();
            let fn_: usize = (0..k).filter(|&j| j != c).map(|j| cm[c * k + j]).sum();
            let p = if tp + fp > 0 {
                tp as f64 / (tp + fp) as f64
            } else {
                0.0
            };
            let r = if tp + fn_ > 0 {
                tp as f64 / (tp + fn_) as f64
            } else {
                0.0
            };
            let f = if p + r > 0.0 {
                2.0 * p * r / (p + r)
            } else {
                0.0
            };
            (p, r, f)
        })
        .collect()
}

/// Linear-interpolation percentile (type 7) of an ascending-sorted slice.
fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return f64::NAN;
    }
    if sorted.len() == 1 {
        return sorted[0];
    }
    let rank = p.clamp(0.0, 1.0) * (sorted.len() - 1) as f64;
    let lo = rank.floor() as usize;
    let hi = (lo + 1).min(sorted.len() - 1);
    let frac = rank - lo as f64;
    sorted[lo] * (1.0 - frac) + sorted[hi] * frac
}

// ── bootstrap metric adapters: evaluate over an index selection ──────────
// These take the row selection directly so replicates never materialise a
// full copy of the table (only macro AUC builds sub-vectors, per class).

type IdxMetric = fn(idx: &[usize], y: &[usize], rows: &[Vec<f64>], k: usize) -> Option<f64>;

fn idx_accuracy(idx: &[usize], y: &[usize], rows: &[Vec<f64>], _k: usize) -> Option<f64> {
    if idx.is_empty() {
        return None;
    }
    let hits = idx.iter().filter(|&&i| argmax(&rows[i]) == y[i]).count();
    Some(hits as f64 / idx.len() as f64)
}

fn idx_balanced_accuracy(idx: &[usize], y: &[usize], rows: &[Vec<f64>], k: usize) -> Option<f64> {
    if idx.is_empty() {
        return None;
    }
    let cm = confusion_idx(idx, y, rows, k);
    ml::metrics::balanced_accuracy(&cm, k).ok()
}

fn idx_macro_auc(idx: &[usize], y: &[usize], rows: &[Vec<f64>], k: usize) -> Option<f64> {
    if idx.is_empty() {
        return None;
    }
    let sub_y: Vec<usize> = idx.iter().map(|&i| y[i]).collect();
    let sub_rows: Vec<Vec<f64>> = idx.iter().map(|&i| rows[i].clone()).collect();
    ml::metrics::macro_ovr_auc(&sub_y, &sub_rows, k).ok()
}

fn idx_brier(idx: &[usize], y: &[usize], rows: &[Vec<f64>], k: usize) -> Option<f64> {
    if idx.is_empty() {
        return None;
    }
    let total: f64 = idx
        .iter()
        .map(|&i| {
            (0..k)
                .map(|c| {
                    let t = if c == y[i] { 1.0 } else { 0.0 };
                    (rows[i][c] - t).powi(2)
                })
                .sum::<f64>()
        })
        .sum();
    Some(total / idx.len() as f64)
}

fn idx_log_loss(idx: &[usize], y: &[usize], rows: &[Vec<f64>], _k: usize) -> Option<f64> {
    if idx.is_empty() {
        return None;
    }
    let total: f64 = idx
        .iter()
        .map(|&i| -rows[i][y[i]].clamp(1e-15, 1.0).ln())
        .sum();
    Some(total / idx.len() as f64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_spec_defaults() {
        let spec: MulticlassMetricsSpec =
            serde_json::from_str(r#"{"label_column":"y","prob_columns":["p_0","p_1","p_2"]}"#)
                .unwrap();
        assert_eq!(spec.label_column, "y");
        assert_eq!(spec.prob_columns.len(), 3);
        assert!(spec.cluster_column.is_none());
        assert_eq!(spec.n_boot, 2000);
        assert_eq!(spec.seed, 0);
        assert!((spec.ci_level - 0.95).abs() < 1e-12);
        assert_eq!(spec.calibration_bins, 10);
    }

    #[test]
    fn test_labels_validation() {
        assert_eq!(
            labels_from_f64(&[0.0, 1.0, 2.0, 1.0], 3).unwrap(),
            vec![0, 1, 2, 1]
        );
        assert!(labels_from_f64(&[0.5, 1.0], 3).is_err()); // non-integer
        assert!(labels_from_f64(&[0.0, 3.0], 3).is_err()); // out of range
        assert!(labels_from_f64(&[0.0, f64::NAN], 3).is_err()); // null → NaN
    }

    #[test]
    fn test_argmax_first_max_tie() {
        assert_eq!(argmax(&[0.2, 0.5, 0.3]), 1);
        assert_eq!(argmax(&[0.5, 0.5, 0.2]), 0);
    }

    #[test]
    fn test_percentile_type7() {
        let v = [1.0, 2.0, 3.0, 4.0, 5.0];
        assert!((percentile(&v, 0.0) - 1.0).abs() < 1e-12);
        assert!((percentile(&v, 0.25) - 2.0).abs() < 1e-12); // rank 1.0 exact
        assert!((percentile(&v, 0.375) - 2.5).abs() < 1e-12); // rank 1.5 midpoint
        assert!((percentile(&v, 0.5) - 3.0).abs() < 1e-12);
        assert!((percentile(&v, 1.0) - 5.0).abs() < 1e-12);
        assert!(percentile(&[], 0.5).is_nan());
        assert!((percentile(&[2.5], 0.9) - 2.5).abs() < 1e-12);
    }

    #[test]
    fn test_per_class_prf_from_cm() {
        // cm row-major [actual][pred]; actual=0: 2 hit 1 miss; actual=1: 3 hit
        let cm = vec![2, 1, 0, 3];
        let prf = per_class_prf(&cm, 2);
        assert!((prf[0].0 - 1.0).abs() < 1e-12); // precision0 = 2/(2+0)
        assert!((prf[0].1 - 2.0 / 3.0).abs() < 1e-12); // recall0
        assert!((prf[1].0 - 3.0 / 4.0).abs() < 1e-12); // precision1 = 3/(3+1)
        assert!((prf[1].1 - 1.0).abs() < 1e-12); // recall1
        assert!((prf[1].2 - 6.0 / 7.0).abs() < 1e-12); // f1_1
    }

    #[test]
    fn test_idx_metrics_consistent_with_library() {
        let y = vec![0usize, 1, 2, 1, 0];
        let rows = vec![
            vec![0.7, 0.2, 0.1],
            vec![0.1, 0.6, 0.3],
            vec![0.2, 0.3, 0.5],
            vec![0.3, 0.4, 0.3],
            vec![0.5, 0.4, 0.1],
        ];
        let all: Vec<usize> = (0..y.len()).collect();
        // argmax preds [0,1,2,1,0] == y → accuracy 1
        assert!((idx_accuracy(&all, &y, &rows, 3).unwrap() - 1.0).abs() < 1e-12);
        assert_eq!(
            confusion_idx(&all, &y, &rows, 3),
            vec![2, 0, 0, 0, 2, 0, 0, 0, 1]
        );
        assert!(
            (idx_brier(&all, &y, &rows, 3).unwrap()
                - ml::metrics::multiclass_brier(&y, &rows, 3).unwrap())
            .abs()
                < 1e-12
        );
        assert!(
            (idx_log_loss(&all, &y, &rows, 3).unwrap()
                - ml::metrics::multiclass_log_loss(&y, &rows, 3).unwrap())
            .abs()
                < 1e-12
        );
        assert!(
            (idx_macro_auc(&all, &y, &rows, 3).unwrap()
                - ml::metrics::macro_ovr_auc(&y, &rows, 3).unwrap())
            .abs()
                < 1e-12
        );
    }
}
