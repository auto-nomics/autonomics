use super::*;

// ═══════════════════════════════════════════════════════════════════════
// Standardize
// ═══════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct StandardizeSpec {
    /// Numeric columns to standardise (z-score).
    pub columns: Vec<String>,
}

pub struct StandardizeFactory;
impl NodeFactory for StandardizeFactory {
    fn kind(&self) -> &'static str {
        "ml_standardize"
    }
    fn desc(&self) -> &'static str {
        "Standardise numeric columns (z-score: subtract mean, divide by std)."
    }
    fn doc(&self) -> &'static str {
        "StandardScaler: for each column, subtract the mean and divide by standard deviation. Constant columns (std=0) cause an error."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(StandardizeSpec)
    }
    fn ports(&self) -> NodePorts {
        NodePorts::new().add_input_port(None).add_output_port(None)
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _ctx: NodeCtx,
    ) -> Result<Box<dyn DagNode>, dag_core::registry::error::Error> {
        let s: StandardizeSpec = serde_json::from_value(spec)?;
        Ok(Box::new(StandardizeNode {
            columns: s.columns,
            meta: self.ports(),
        }))
    }
}

#[derive(Clone)]
struct StandardizeNode {
    columns: Vec<String>,
    meta: NodePorts,
}

#[async_trait]
impl DagNode for StandardizeNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }
    fn kind(&self) -> &'static str {
        "ml_standardize"
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
        let (_, transformed) =
            StandardScaler::fit_transform(&data).map_err(|e| DagError::NodeError {
                node_type: "ml_standardize".into(),
                msg: e.to_string(),
            })?;
        let batch = replace_columns(&batches, &self.columns, &transformed, &[])?;
        emit_batch(ctx, batch)
    }
}
