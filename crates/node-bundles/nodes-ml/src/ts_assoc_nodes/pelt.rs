use super::*;

// ═══════════════════════════════════════════════════════════════════════
// Change-point detection (PELT)
// ═══════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct PeltSpec {
    pub value_column: String,
    #[serde(default = "d_pelt_penalty")]
    pub penalty: f64,
}
fn d_pelt_penalty() -> f64 {
    10.0
}

pub struct PeltFactory;
impl NodeFactory for PeltFactory {
    fn kind(&self) -> &'static str {
        "ml_changepoint"
    }
    fn desc(&self) -> &'static str {
        "Change-point detection (PELT algorithm)."
    }
    fn doc(&self) -> &'static str {
        "PELT: detects change-points in a time series by minimizing segment cost + penalty. Returns detected change-point indices."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(PeltSpec)
    }
    fn ports(&self) -> NodePorts {
        NodePorts::new().add_input_port(None).add_output_port(None)
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _ctx: NodeCtx,
    ) -> Result<Box<dyn DagNode>, dag_core::registry::error::Error> {
        let s: PeltSpec = serde_json::from_value(spec)?;
        Ok(Box::new(PeltNode {
            value_column: s.value_column,
            penalty: s.penalty,
            meta: self.ports(),
        }))
    }
}

#[derive(Clone)]
struct PeltNode {
    value_column: String,
    penalty: f64,
    meta: NodePorts,
}

#[async_trait]
impl DagNode for PeltNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }
    fn kind(&self) -> &'static str {
        "ml_changepoint"
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
                node_type: "ml_changepoint".into(),
                msg: e.to_string(),
            }
        })?;
        let cps = ml::timeseries::pelt(&data, self.penalty).map_err(|e| DagError::NodeError {
            node_type: "ml_changepoint".into(),
            msg: e.to_string(),
        })?;
        let batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![Field::new(
                "changepoint_index",
                DataType::UInt32,
                false,
            )])),
            vec![Arc::new(arrow_array::UInt32Array::from(
                cps.iter().map(|&c| c as u32).collect::<Vec<_>>(),
            ))],
        )
        .map_err(|e| DagError::NodeError {
            node_type: "ml_changepoint".into(),
            msg: e.to_string(),
        })?;
        emit_batch(ctx, batch)
    }
}
