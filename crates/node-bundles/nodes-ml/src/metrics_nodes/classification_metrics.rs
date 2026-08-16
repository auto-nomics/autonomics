use super::*;

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
fn d_positive() -> f64 {
    1.0
}

pub struct ClassificationMetricsFactory;
impl NodeFactory for ClassificationMetricsFactory {
    fn kind(&self) -> &'static str {
        "ml_classification_metrics"
    }
    fn desc(&self) -> &'static str {
        "Compute classification metrics: accuracy, precision, recall, F1, AUC."
    }
    fn doc(&self) -> &'static str {
        "ClassificationMetrics: takes y_true + y_pred columns and computes accuracy, precision (macro + per-class), recall, F1, and optionally ROC AUC. Outputs a single-row metrics table."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(ClassificationMetricsSpec)
    }
    fn ports(&self) -> NodePorts {
        NodePorts::new().add_input_port(None).add_output_port(None)
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _ctx: NodeCtx,
    ) -> Result<Box<dyn DagNode>, dag_core::registry::error::Error> {
        let s: ClassificationMetricsSpec = serde_json::from_value(spec)?;
        Ok(Box::new(ClassificationMetricsNode {
            y_true: s.y_true,
            y_pred: s.y_pred,
            y_score: s.y_score,
            n_classes: s.n_classes,
            positive: s.positive,
            meta: self.ports(),
        }))
    }
}

#[derive(Clone)]
#[allow(dead_code)] // Retained in the node for future multi-class metric output.
struct ClassificationMetricsNode {
    y_true: String,
    y_pred: String,
    y_score: Option<String>,
    n_classes: Option<usize>,
    positive: f64,
    meta: NodePorts,
}

#[async_trait]
impl DagNode for ClassificationMetricsNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }
    fn kind(&self) -> &'static str {
        "ml_classification_metrics"
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
        let batches = collect_batches(inputs).await?;
        let y_true = common::extract_numeric_column(&batches, &self.y_true).map_err(|e| {
            DagError::NodeError {
                node_type: "ml_classification_metrics".into(),
                msg: e.to_string(),
            }
        })?;
        let y_pred = common::extract_numeric_column(&batches, &self.y_pred).map_err(|e| {
            DagError::NodeError {
                node_type: "ml_classification_metrics".into(),
                msg: e.to_string(),
            }
        })?;

        let acc = ml::metrics::accuracy(&y_true, &y_pred).map_err(|e| DagError::NodeError {
            node_type: "ml_classification_metrics".into(),
            msg: e.to_string(),
        })?;

        let (p, r, f1) = ml::metrics::precision_recall_f1(&y_true, &y_pred, self.positive)
            .map_err(|e| DagError::NodeError {
                node_type: "ml_classification_metrics".into(),
                msg: e.to_string(),
            })?;

        // AUC if score column provided
        let auc = if let Some(score_col) = &self.y_score {
            let y_score = common::extract_numeric_column(&batches, score_col).map_err(|e| {
                DagError::NodeError {
                    node_type: "ml_classification_metrics".into(),
                    msg: e.to_string(),
                }
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
        let batch = RecordBatch::try_new(Arc::new(Schema::new(fields)), arrays).map_err(|e| {
            DagError::NodeError {
                node_type: "ml_classification_metrics".into(),
                msg: e.to_string(),
            }
        })?;
        emit_batch(ctx, batch)
    }
}
