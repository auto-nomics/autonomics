//! Metrics DAG nodes — classification & regression evaluation.

use std::sync::Arc;

use arrow_array::{Array, Float64Array, RecordBatch};
use arrow_schema::{DataType, Field, Schema};
use async_trait::async_trait;
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;

use dag_core::node::{DagNode, NodeInput, NodePorts};
use super::common;
use dag_core::dag::{DagError, graph::PortOutputs};
use dag_core::registry::{NodeCtx, NodeFactory};

// ═══════════════════════════════════════════════════════════════════════
// ClassificationMetrics
// ═══════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct ClassificationMetricsSpec {
    /// Column with ground-truth integer class labels.
    pub y_true: String,
    /// Column with predicted class labels.
    pub y_pred: String,
    /// Optional column with predicted probability of the positive class (for AUC).
    #[serde(default)]
    pub y_score: Option<String>,
    /// Number of classes (for multi-class metrics). If omitted, inferred.
    #[serde(default)]
    pub n_classes: Option<usize>,
    /// Positive class label for binary precision/recall/F1. Default 1.
    #[serde(default = "d_positive")]
    pub positive: f64,
}
fn d_positive() -> f64 { 1.0 }

pub struct ClassificationMetricsFactory;
impl NodeFactory for ClassificationMetricsFactory {
    fn kind(&self) -> &'static str { "ml_classification_metrics" }
    fn desc(&self) -> &'static str { "Compute classification metrics: accuracy, precision, recall, F1, AUC." }
    fn doc(&self) -> &'static str { "ClassificationMetrics: takes y_true + y_pred columns and computes accuracy, precision (macro + per-class), recall, F1, and optionally ROC AUC. Outputs a single-row metrics table." }
    fn spec_schema(&self) -> schemars::Schema { schema_for!(ClassificationMetricsSpec) }
    fn ports(&self) -> NodePorts { NodePorts::new().add_input_port(None).add_output_port(None) }
    fn build(&self, spec: serde_json::Value, _ctx: NodeCtx) -> Result<Box<dyn DagNode>, dag_core::registry::error::Error> {
        let s: ClassificationMetricsSpec = serde_json::from_value(spec)?;
        Ok(Box::new(ClassificationMetricsNode {
            y_true: s.y_true, y_pred: s.y_pred, y_score: s.y_score,
            n_classes: s.n_classes, positive: s.positive, meta: self.ports(),
        }))
    }
}

#[derive(Clone)]
struct ClassificationMetricsNode {
    y_true: String, y_pred: String, y_score: Option<String>,
    n_classes: Option<usize>, positive: f64, meta: NodePorts,
}

#[async_trait]
impl DagNode for ClassificationMetricsNode {
    fn ports(&self) -> &NodePorts { &self.meta }
    fn clone_box(&self) -> Box<dyn DagNode> { Box::new(self.clone()) }
    fn kind(&self) -> &'static str { "ml_classification_metrics" }
    fn as_any(&self) -> &dyn std::any::Any { self }
    async fn execute(&mut self, ctx: &NodeCtx, inputs: &[NodeInput], _r: &dag_core::dag::node_event::NodeReporter) -> Result<PortOutputs, DagError> {
        let batches = collect_batches(inputs).await?;
        let y_true = common::extract_numeric_column(&batches, &self.y_true).map_err(|e| DagError::NodeError {
            node_type: "ml_classification_metrics".into(), msg: e.to_string()
        })?;
        let y_pred = common::extract_numeric_column(&batches, &self.y_pred).map_err(|e| DagError::NodeError {
            node_type: "ml_classification_metrics".into(), msg: e.to_string()
        })?;

        let acc = ml::metrics::accuracy(&y_true, &y_pred).map_err(|e| DagError::NodeError {
            node_type: "ml_classification_metrics".into(), msg: e.to_string()
        })?;

        let (p, r, f1) = ml::metrics::precision_recall_f1(&y_true, &y_pred, self.positive)
            .map_err(|e| DagError::NodeError { node_type: "ml_classification_metrics".into(), msg: e.to_string() })?;

        // AUC if score column provided
        let auc = if let Some(score_col) = &self.y_score {
            let y_score = common::extract_numeric_column(&batches, score_col).map_err(|e| DagError::NodeError {
                node_type: "ml_classification_metrics".into(), msg: e.to_string()
            })?;
            Some(ml::metrics::roc_auc(&y_true, &y_score).unwrap_or(f64::NAN))
        } else {
            None
        };

        let n = 1usize;
        let mut fields = vec![
            Arc::new(Field::new("accuracy", DataType::Float64, false)),
            Arc::new(Field::new("precision", DataType::Float64, false)),
            Arc::new(Field::new("recall", DataType::Float64, false)),
            Arc::new(Field::new("f1", DataType::Float64, false)),
            Arc::new(Field::new("n_samples", DataType::Float64, false)),
        ];
        let mut arrays: Vec<Arc<dyn Array>> = vec![
            Arc::new(Float64Array::from(vec![acc])),
            Arc::new(Float64Array::from(vec![p])),
            Arc::new(Float64Array::from(vec![r])),
            Arc::new(Float64Array::from(vec![f1])),
            Arc::new(Float64Array::from(vec![y_true.len() as f64])),
        ];
        if let Some(auc_val) = auc {
            fields.push(Arc::new(Field::new("roc_auc", DataType::Float64, false)));
            arrays.push(Arc::new(Float64Array::from(vec![auc_val])));
        }
        let _ = n;
        let batch = RecordBatch::try_new(Arc::new(Schema::new(fields)), arrays).map_err(|e| DagError::NodeError {
            node_type: "ml_classification_metrics".into(), msg: e.to_string()
        })?;
        emit_batch(ctx, batch)
    }
}

// ═══════════════════════════════════════════════════════════════════════
// RegressionMetrics
// ═══════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct RegressionMetricsSpec {
    pub y_true: String,
    pub y_pred: String,
}

pub struct RegressionMetricsFactory;
impl NodeFactory for RegressionMetricsFactory {
    fn kind(&self) -> &'static str { "ml_regression_metrics" }
    fn desc(&self) -> &'static str { "Compute regression metrics: MSE, RMSE, MAE, R², MAPE." }
    fn doc(&self) -> &'static str { "RegressionMetrics: takes y_true + y_pred columns and computes MSE, RMSE, MAE, R², MAPE, explained variance, and max error." }
    fn spec_schema(&self) -> schemars::Schema { schema_for!(RegressionMetricsSpec) }
    fn ports(&self) -> NodePorts { NodePorts::new().add_input_port(None).add_output_port(None) }
    fn build(&self, spec: serde_json::Value, _ctx: NodeCtx) -> Result<Box<dyn DagNode>, dag_core::registry::error::Error> {
        let s: RegressionMetricsSpec = serde_json::from_value(spec)?;
        Ok(Box::new(RegressionMetricsNode { y_true: s.y_true, y_pred: s.y_pred, meta: self.ports() }))
    }
}

#[derive(Clone)]
struct RegressionMetricsNode { y_true: String, y_pred: String, meta: NodePorts }

#[async_trait]
impl DagNode for RegressionMetricsNode {
    fn ports(&self) -> &NodePorts { &self.meta }
    fn clone_box(&self) -> Box<dyn DagNode> { Box::new(self.clone()) }
    fn kind(&self) -> &'static str { "ml_regression_metrics" }
    fn as_any(&self) -> &dyn std::any::Any { self }
    async fn execute(&mut self, ctx: &NodeCtx, inputs: &[NodeInput], _r: &dag_core::dag::node_event::NodeReporter) -> Result<PortOutputs, DagError> {
        let batches = collect_batches(inputs).await?;
        let y_true = common::extract_numeric_column(&batches, &self.y_true).map_err(|e| DagError::NodeError {
            node_type: "ml_regression_metrics".into(), msg: e.to_string()
        })?;
        let y_pred = common::extract_numeric_column(&batches, &self.y_pred).map_err(|e| DagError::NodeError {
            node_type: "ml_regression_metrics".into(), msg: e.to_string()
        })?;

        let mse = ml::metrics::mse(&y_true, &y_pred).map_err(|e| DagError::NodeError { node_type: "ml_regression_metrics".into(), msg: e.to_string() })?;
        let rmse = ml::metrics::rmse(&y_true, &y_pred).map_err(|e| DagError::NodeError { node_type: "ml_regression_metrics".into(), msg: e.to_string() })?;
        let mae = ml::metrics::mae(&y_true, &y_pred).map_err(|e| DagError::NodeError { node_type: "ml_regression_metrics".into(), msg: e.to_string() })?;
        let r2 = ml::metrics::r2_score(&y_true, &y_pred).map_err(|e| DagError::NodeError { node_type: "ml_regression_metrics".into(), msg: e.to_string() })?;
        let mape = ml::metrics::mape(&y_true, &y_pred).map_err(|e| DagError::NodeError { node_type: "ml_regression_metrics".into(), msg: e.to_string() })?;
        let ev = ml::metrics::explained_variance(&y_true, &y_pred).map_err(|e| DagError::NodeError { node_type: "ml_regression_metrics".into(), msg: e.to_string() })?;
        let me = ml::metrics::max_error(&y_true, &y_pred).map_err(|e| DagError::NodeError { node_type: "ml_regression_metrics".into(), msg: e.to_string() })?;

        let fields = vec![
            Arc::new(Field::new("mse", DataType::Float64, false)),
            Arc::new(Field::new("rmse", DataType::Float64, false)),
            Arc::new(Field::new("mae", DataType::Float64, false)),
            Arc::new(Field::new("r2", DataType::Float64, false)),
            Arc::new(Field::new("mape", DataType::Float64, false)),
            Arc::new(Field::new("explained_variance", DataType::Float64, false)),
            Arc::new(Field::new("max_error", DataType::Float64, false)),
            Arc::new(Field::new("n_samples", DataType::Float64, false)),
        ];
        let arrays: Vec<Arc<dyn Array>> = vec![
            Arc::new(Float64Array::from(vec![mse])),
            Arc::new(Float64Array::from(vec![rmse])),
            Arc::new(Float64Array::from(vec![mae])),
            Arc::new(Float64Array::from(vec![r2])),
            Arc::new(Float64Array::from(vec![mape])),
            Arc::new(Float64Array::from(vec![ev])),
            Arc::new(Float64Array::from(vec![me])),
            Arc::new(Float64Array::from(vec![y_true.len() as f64])),
        ];
        let batch = RecordBatch::try_new(Arc::new(Schema::new(fields)), arrays).map_err(|e| DagError::NodeError {
            node_type: "ml_regression_metrics".into(), msg: e.to_string()
        })?;
        emit_batch(ctx, batch)
    }
}

// ── helpers ──────────────────────────────────────────────────────────────

async fn collect_batches(inputs: &[NodeInput]) -> Result<Vec<RecordBatch>, DagError> {
    let input = inputs.first().ok_or(DagError::NodeError {
        node_type: "ml_metrics".into(), msg: "no input".into()
    })?;
    input.data.clone().collect().await.map_err(|e| DagError::NodeError {
        node_type: "ml_metrics".into(), msg: format!("collect: {e}"),
    })
}

fn emit_batch(ctx: &NodeCtx, batch: RecordBatch) -> Result<PortOutputs, DagError> {
    let df = ctx.session().read_batch(batch).map_err(|e| DagError::NodeError {
        node_type: "ml_metrics".into(), msg: format!("read_batch: {e}"),
    })?;
    let mut res = PortOutputs::new();
    res.insert(0, df);
    Ok(res)
}
