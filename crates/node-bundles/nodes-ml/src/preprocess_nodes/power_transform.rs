use super::*;

// ═══════════════════════════════════════════════════════════════════════
// PowerTransform
// ═══════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct PowerTransformSpec {
    pub columns: Vec<String>,
}

pub struct PowerTransformFactory;
impl NodeFactory for PowerTransformFactory {
    fn kind(&self) -> &'static str {
        "ml_power_transform"
    }
    fn desc(&self) -> &'static str {
        "Yeo-Johnson power transform for approximate normality."
    }
    fn doc(&self) -> &'static str {
        "PowerTransformer: fits Yeo-Johnson λ per column via MLE, transforms data to approximate Gaussian. Works on positive and negative values."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(PowerTransformSpec)
    }
    fn ports(&self) -> NodePorts {
        NodePorts::new().add_input_port(None).add_output_port(None)
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _ctx: NodeCtx,
    ) -> Result<Box<dyn DagNode>, dag_core::registry::error::Error> {
        let s: PowerTransformSpec = serde_json::from_value(spec)?;
        Ok(Box::new(PowerTransformNode {
            columns: s.columns,
            meta: self.ports(),
        }))
    }
}

#[derive(Clone)]
struct PowerTransformNode {
    columns: Vec<String>,
    meta: NodePorts,
}

#[async_trait]
impl DagNode for PowerTransformNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }
    fn kind(&self) -> &'static str {
        "ml_power_transform"
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
        let scaler = PowerTransformer::fit(&data).map_err(|e| DagError::NodeError {
            node_type: "ml_power_transform".into(),
            msg: e.to_string(),
        })?;
        let mut transformed = data;
        scaler.transform(&mut transformed);
        let batch = replace_columns(&batches, &self.columns, &transformed, &[])?;
        emit_batch(ctx, batch)
    }
}
