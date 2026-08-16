use super::*;

// ═══════════════════════════════════════════════════════════════════════
// STL Decomposition
// ═══════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct StlSpec {
    pub value_column: String,
    pub period: usize,
}

pub struct StlFactory;
impl NodeFactory for StlFactory {
    fn kind(&self) -> &'static str {
        "ml_stl_decompose"
    }
    fn desc(&self) -> &'static str {
        "STL decomposition (trend + seasonal + residual)."
    }
    fn doc(&self) -> &'static str {
        "STL: decomposes a time series into trend, seasonal, and residual components using moving-average trend extraction."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(StlSpec)
    }
    fn ports(&self) -> NodePorts {
        NodePorts::new().add_input_port(None).add_output_port(None)
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _ctx: NodeCtx,
    ) -> Result<Box<dyn DagNode>, dag_core::registry::error::Error> {
        let s: StlSpec = serde_json::from_value(spec)?;
        Ok(Box::new(StlNode {
            value_column: s.value_column,
            period: s.period,
            meta: self.ports(),
        }))
    }
}

#[derive(Clone)]
struct StlNode {
    value_column: String,
    period: usize,
    meta: NodePorts,
}

#[async_trait]
impl DagNode for StlNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }
    fn kind(&self) -> &'static str {
        "ml_stl_decompose"
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
        let data = common::extract_numeric_column(&batches, &self.value_column).map_err(|e| {
            DagError::NodeError {
                node_type: "ml_stl_decompose".into(),
                msg: e.to_string(),
            }
        })?;
        let result =
            ml::timeseries::stl_decompose(&data, self.period).map_err(|e| DagError::NodeError {
                node_type: "ml_stl_decompose".into(),
                msg: e.to_string(),
            })?;
        let batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("trend", DataType::Float64, false),
                Field::new("seasonal", DataType::Float64, false),
                Field::new("residual", DataType::Float64, false),
            ])),
            vec![
                Arc::new(Float64Array::from(result.trend)),
                Arc::new(Float64Array::from(result.seasonal)),
                Arc::new(Float64Array::from(result.residual)),
            ],
        )
        .map_err(|e| DagError::NodeError {
            node_type: "ml_stl_decompose".into(),
            msg: e.to_string(),
        })?;
        emit_batch(ctx, batch)
    }
}
