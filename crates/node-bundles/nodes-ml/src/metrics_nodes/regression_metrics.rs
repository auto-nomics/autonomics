use super::*;

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
    fn kind(&self) -> &'static str {
        "ml_regression_metrics"
    }
    fn desc(&self) -> &'static str {
        "Compute regression metrics: MSE, RMSE, MAE, R², MAPE."
    }
    fn doc(&self) -> &'static str {
        "RegressionMetrics: takes y_true + y_pred columns and computes MSE, RMSE, MAE, R², MAPE, explained variance, and max error."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(RegressionMetricsSpec)
    }
    fn ports(&self) -> NodePorts {
        NodePorts::new().add_input_port(None).add_output_port(None)
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _ctx: NodeCtx,
    ) -> Result<Box<dyn DagNode>, dag_core::registry::error::Error> {
        let s: RegressionMetricsSpec = serde_json::from_value(spec)?;
        Ok(Box::new(RegressionMetricsNode {
            y_true: s.y_true,
            y_pred: s.y_pred,
            meta: self.ports(),
        }))
    }
}

#[derive(Clone)]
struct RegressionMetricsNode {
    y_true: String,
    y_pred: String,
    meta: NodePorts,
}

#[async_trait]
impl DagNode for RegressionMetricsNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }
    fn kind(&self) -> &'static str {
        "ml_regression_metrics"
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
                node_type: "ml_regression_metrics".into(),
                msg: e.to_string(),
            }
        })?;
        let y_pred = common::extract_numeric_column(&batches, &self.y_pred).map_err(|e| {
            DagError::NodeError {
                node_type: "ml_regression_metrics".into(),
                msg: e.to_string(),
            }
        })?;

        let mse = ml::metrics::mse(&y_true, &y_pred).map_err(|e| DagError::NodeError {
            node_type: "ml_regression_metrics".into(),
            msg: e.to_string(),
        })?;
        let rmse = ml::metrics::rmse(&y_true, &y_pred).map_err(|e| DagError::NodeError {
            node_type: "ml_regression_metrics".into(),
            msg: e.to_string(),
        })?;
        let mae = ml::metrics::mae(&y_true, &y_pred).map_err(|e| DagError::NodeError {
            node_type: "ml_regression_metrics".into(),
            msg: e.to_string(),
        })?;
        let r2 = ml::metrics::r2_score(&y_true, &y_pred).map_err(|e| DagError::NodeError {
            node_type: "ml_regression_metrics".into(),
            msg: e.to_string(),
        })?;
        let mape = ml::metrics::mape(&y_true, &y_pred).map_err(|e| DagError::NodeError {
            node_type: "ml_regression_metrics".into(),
            msg: e.to_string(),
        })?;
        let ev =
            ml::metrics::explained_variance(&y_true, &y_pred).map_err(|e| DagError::NodeError {
                node_type: "ml_regression_metrics".into(),
                msg: e.to_string(),
            })?;
        let me = ml::metrics::max_error(&y_true, &y_pred).map_err(|e| DagError::NodeError {
            node_type: "ml_regression_metrics".into(),
            msg: e.to_string(),
        })?;

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
        let batch = RecordBatch::try_new(Arc::new(Schema::new(fields)), arrays).map_err(|e| {
            DagError::NodeError {
                node_type: "ml_regression_metrics".into(),
                msg: e.to_string(),
            }
        })?;
        emit_batch(ctx, batch)
    }
}
