use super::*;

// ═══════════════════════════════════════════════════════════════════════
// RobustScale
// ═══════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct RobustScaleSpec {
    pub columns: Vec<String>,
}

pub struct RobustScaleFactory;
impl NodeFactory for RobustScaleFactory {
    fn kind(&self) -> &'static str {
        "ml_robust_scale"
    }
    fn desc(&self) -> &'static str {
        "Scale using median and IQR (robust to outliers)."
    }
    fn doc(&self) -> &'static str {
        "RobustScaler: subtract median and divide by interquartile range (Q3 - Q1). Outlier-resistant."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(RobustScaleSpec)
    }
    fn ports(&self) -> NodePorts {
        NodePorts::new().add_input_port(None).add_output_port(None)
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _ctx: NodeCtx,
    ) -> Result<Box<dyn DagNode>, dag_core::registry::error::Error> {
        let s: RobustScaleSpec = serde_json::from_value(spec)?;
        Ok(Box::new(RobustScaleNode {
            columns: s.columns,
            meta: self.ports(),
        }))
    }
}

#[derive(Clone)]
struct RobustScaleNode {
    columns: Vec<String>,
    meta: NodePorts,
}

#[async_trait]
impl DagNode for RobustScaleNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }
    fn kind(&self) -> &'static str {
        "ml_robust_scale"
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
        let data = extract_features(&batches, &self.columns)?;
        let scaler = RobustScaler::fit(&data).map_err(|e| DagError::NodeError {
            node_type: "ml_robust_scale".into(),
            msg: e.to_string(),
        })?;
        let mut transformed = data;
        scaler.transform(&mut transformed);
        let batch = replace_columns(&batches, &self.columns, &transformed, &[])?;
        emit_batch(ctx, batch)
    }
}
