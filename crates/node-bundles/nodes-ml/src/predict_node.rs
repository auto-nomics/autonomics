//! `ml_predict` — train-on-port-0 / predict-on-port-1 classifier node.
//!
//! All `ml_*` classification nodes train and predict on the same data,
//! producing optimistic (in-sample) AUC.  This node takes **training data**
//! on port 0 and **test data** on port 1, trains the specified model on the
//! training data, then predicts on the test data — giving unbiased hold-out
//! metrics when fed to `ml_classification_metrics` or `epi_roc`.
//!
//! Supported `model_kind` values: `logistic`, `gaussian_nb`, `knn`,
//! `svm`, `adaboost`, `decision_tree`. The `probability` output is
//! `P(class=1)`, not the confidence of the predicted class.

use std::sync::Arc;

use arrow_array::{Array, Float64Array, RecordBatch, UInt32Array};
use arrow_schema::{DataType, Field, Schema};
use async_trait::async_trait;
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;

use super::common;
use dag_core::dag::{DagError, graph::PortOutputs};
use dag_core::node::{DagNode, NodeInput, NodePorts};
use dag_core::registry::{NodeCtx, NodeFactory};

const NODE: &str = "ml_predict";

// ── helpers ──────────────────────────────────────────────────────────────

async fn collect_port(inputs: &[NodeInput], port: u8) -> Result<Vec<RecordBatch>, DagError> {
    let input = inputs
        .iter()
        .find(|i| i.port == port)
        .ok_or_else(|| DagError::NodeError {
            node_type: NODE.into(),
            msg: format!("input port {port} not connected"),
        })?;
    input
        .dataframe()?
        .clone()
        .collect()
        .await
        .map_err(|e| DagError::NodeError {
            node_type: NODE.into(),
            msg: format!("collect port {port}: {e}"),
        })
}

fn err(msg: impl Into<String>) -> DagError {
    DagError::NodeError {
        node_type: NODE.into(),
        msg: msg.into(),
    }
}

/// Lightweight per-model hyper-parameter spec.  Only the fields relevant to
/// the chosen `model_kind` are used; others are ignored.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct PredictSpec {
    /// Which classifier to train.  Must match the algorithm used upstream
    /// (or the one you intend to evaluate).
    pub model_kind: ModelKind,
    pub features: Vec<String>,
    pub label_column: String,
    /// Logistic L2 strength (default 0.1).
    #[serde(default = "d_alpha")]
    pub alpha: f64,
    /// Logistic max gradient-descent iterations (default 200).
    #[serde(default = "d_max_iter")]
    pub max_iter: usize,
    /// KNN neighbours (default 5).
    #[serde(default = "d_k")]
    pub k: usize,
    /// SVM kernel: "linear" | "rbf" | "poly" (default "rbf").
    #[serde(default = "d_svm_kernel")]
    pub kernel: String,
    /// SVM regularisation parameter C (default 1.0).
    #[serde(default = "d_svm_c")]
    pub c: f64,
    /// SVM RBF kernel width γ. `null` (default) uses the "scale" heuristic
    /// `1/(n_features · Var(X))`.
    #[serde(default)]
    pub gamma: Option<f64>,
    /// AdaBoost number of estimators (default 50).
    #[serde(default = "d_ab_n")]
    pub n_estimators: usize,
    /// AdaBoost learning rate (default 1.0).
    #[serde(default = "d_ab_lr")]
    pub learning_rate: f64,
    /// Decision tree max depth (default 10).
    #[serde(default = "d_dt_depth")]
    pub max_depth: usize,
    /// Decision tree min samples per split (default 2).
    #[serde(default = "d_dt_split")]
    pub min_samples_split: usize,
    /// Decision tree min samples per leaf (default 1).
    #[serde(default = "d_dt_leaf")]
    pub min_samples_leaf: usize,
}

#[derive(Debug, Clone, Copy, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ModelKind {
    Logistic,
    GaussianNb,
    Knn,
    Svm,
    Adaboost,
    DecisionTree,
}

fn d_alpha() -> f64 {
    0.1
}
fn d_max_iter() -> usize {
    200
}
fn d_k() -> usize {
    5
}
fn d_svm_kernel() -> String {
    "rbf".into()
}
fn d_svm_c() -> f64 {
    1.0
}
fn d_ab_n() -> usize {
    50
}
fn d_ab_lr() -> f64 {
    1.0
}
fn d_dt_depth() -> usize {
    10
}
fn d_dt_split() -> usize {
    2
}
fn d_dt_leaf() -> usize {
    1
}

// ═══════════════════════════════════════════════════════════════════════
// Factory
// ═══════════════════════════════════════════════════════════════════════

pub struct PredictFactory;
impl NodeFactory for PredictFactory {
    fn kind(&self) -> &'static str {
        NODE
    }
    fn desc(&self) -> &'static str {
        "Train on port-0 data and predict on port-1 data; probability is P(class=1)."
    }
    fn doc(&self) -> &'static str {
        "ml_predict: takes training data (port 0) and test data (port 1), trains the specified model on training data, then predicts on test data. Enables proper train:test separation for unbiased evaluation. Supported models: logistic, gaussian_nb, knn, svm, adaboost, decision_tree. The probability output is P(class=1)."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(PredictSpec)
    }
    fn ports(&self) -> NodePorts {
        NodePorts::new()
            .add_input_port(None) // port 0 — training data
            .add_input_port(None) // port 1 — test data
            .add_output_port(None) // port 0 — test data + prediction + probability
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _ctx: NodeCtx,
    ) -> Result<Box<dyn DagNode>, dag_core::registry::error::Error> {
        let s: PredictSpec = serde_json::from_value(spec)?;
        Ok(Box::new(PredictNode {
            spec: s,
            meta: self.ports(),
        }))
    }
}

#[derive(Clone)]
struct PredictNode {
    spec: PredictSpec,
    meta: NodePorts,
}

/// A simple struct holding prediction + probability vectors, uniform across
/// all classifier types.
struct PredictResult {
    predictions: Vec<usize>,
    probabilities: Vec<f64>,
}

#[async_trait]
impl DagNode for PredictNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }
    fn kind(&self) -> &'static str {
        NODE
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
        // ── Collect training data (port 0) ───────────────────────────────
        let train_batches = collect_port(inputs, 0).await?;
        // ── Collect test data (port 1) ───────────────────────────────────
        let test_batches = collect_port(inputs, 1).await?;

        // ── Extract matrices ─────────────────────────────────────────────
        let train_x = common::extract_matrix(&train_batches, &self.spec.features)
            .map_err(|e| err(e.to_string()))?;
        let train_labels_f =
            common::extract_numeric_column(&train_batches, &self.spec.label_column)
                .map_err(|e| err(e.to_string()))?;
        let train_labels: Vec<usize> = train_labels_f.into_iter().map(|v| v as usize).collect();

        let test_x = common::extract_matrix(&test_batches, &self.spec.features)
            .map_err(|e| err(e.to_string()))?;

        // ── Train on train, predict on test ──────────────────────────────
        let result = match self.spec.model_kind {
            ModelKind::Logistic => {
                let r = ml::classify::logistic_fit_predict(
                    &train_x,
                    &train_labels,
                    &test_x,
                    self.spec.alpha,
                    self.spec.max_iter,
                )
                .map_err(|e| err(e.to_string()))?;
                PredictResult {
                    predictions: r.predictions,
                    probabilities: r.probabilities,
                }
            }
            ModelKind::GaussianNb => {
                let r = ml::classify::gaussian_nb_fit_predict(&train_x, &train_labels, &test_x)
                    .map_err(|e| err(e.to_string()))?;
                PredictResult {
                    predictions: r.predictions,
                    probabilities: r.probabilities,
                }
            }
            ModelKind::Knn => {
                let r = ml::classify::knn_classify(&train_x, &train_labels, &test_x, self.spec.k)
                    .map_err(|e| err(e.to_string()))?;
                PredictResult {
                    predictions: r.predictions,
                    probabilities: r.probabilities,
                }
            }
            ModelKind::Svm => {
                let r = ml::svm_ensemble::svm_fit_predict(
                    &train_x,
                    &train_labels,
                    &test_x,
                    &self.spec.kernel,
                    self.spec.c,
                    self.spec.gamma,
                )
                .map_err(|e| err(e.to_string()))?;
                PredictResult {
                    predictions: r.predictions,
                    probabilities: r.probabilities,
                }
            }
            ModelKind::Adaboost => {
                let r = ml::svm_ensemble::adaboost_fit_predict(
                    &train_x,
                    &train_labels,
                    &test_x,
                    self.spec.n_estimators,
                    self.spec.learning_rate,
                )
                .map_err(|e| err(e.to_string()))?;
                PredictResult {
                    predictions: r.predictions,
                    probabilities: r.probabilities,
                }
            }
            ModelKind::DecisionTree => {
                let r = ml::classify::decision_tree_fit_predict(
                    &train_x,
                    &train_labels,
                    &test_x,
                    self.spec.max_depth,
                    self.spec.min_samples_split,
                    self.spec.min_samples_leaf,
                )
                .map_err(|e| err(e.to_string()))?;
                PredictResult {
                    predictions: r.predictions,
                    probabilities: r.probabilities,
                }
            }
        };

        // ── Build output: test data + prediction + probability ───────────
        let (_schema, mut fields, mut arrays) = common::concat_input(&test_batches)?;
        fields.push(Arc::new(Field::new("prediction", DataType::UInt32, false)));
        arrays.push(Arc::new(UInt32Array::from(
            result
                .predictions
                .iter()
                .map(|&p| p as u32)
                .collect::<Vec<_>>(),
        )));
        fields.push(Arc::new(Field::new(
            "probability",
            DataType::Float64,
            false,
        )));
        arrays.push(Arc::new(Float64Array::from(result.probabilities)));

        let batch = RecordBatch::try_new(Arc::new(Schema::new(fields)), arrays)
            .map_err(|e| err(format!("build output batch: {e}")))?;

        let df = ctx
            .session()
            .read_batch(batch)
            .map_err(|e| err(format!("read_batch: {e}")))?;
        let mut res = PortOutputs::new();
        res.insert(0, df);
        Ok(res)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_model_kind_deserialize() {
        let json = r#""logistic""#;
        let kind: ModelKind = serde_json::from_str(json).unwrap();
        assert!(matches!(kind, ModelKind::Logistic));

        let json = r#""gaussian_nb""#;
        let kind: ModelKind = serde_json::from_str(json).unwrap();
        assert!(matches!(kind, ModelKind::GaussianNb));

        let json = r#""decision_tree""#;
        let kind: ModelKind = serde_json::from_str(json).unwrap();
        assert!(matches!(kind, ModelKind::DecisionTree));
    }

    #[test]
    fn test_spec_deserialize() {
        let json = r#"{
            "model_kind": "knn",
            "features": ["a", "b"],
            "label_column": "y",
            "k": 7
        }"#;
        let spec: PredictSpec = serde_json::from_str(json).unwrap();
        assert!(matches!(spec.model_kind, ModelKind::Knn));
        assert_eq!(spec.features, vec!["a", "b"]);
        assert_eq!(spec.label_column, "y");
        assert_eq!(spec.k, 7);
        // Defaults should fill in
        assert_eq!(spec.alpha, 0.1);
        assert_eq!(spec.kernel, "rbf");
    }
}
