use super::*;

// ═══════════════════════════════════════════════════════════════════════
// NormalizeRows
// ═══════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct NormalizeRowsSpec {
    pub columns: Vec<String>,
    #[serde(default = "default_norm")]
    pub norm: String,
}
fn default_norm() -> String {
    "l2".into()
}

pub struct NormalizeRowsFactory;
impl NodeFactory for NormalizeRowsFactory {
    fn kind(&self) -> &'static str {
        "ml_normalize_rows"
    }
    fn desc(&self) -> &'static str {
        "Normalise each row to unit norm (L1, L2, or Max)."
    }
    fn doc(&self) -> &'static str {
        "Row-wise normalisation: each row vector is scaled so its L1/L2/Max norm equals 1."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(NormalizeRowsSpec)
    }
    fn ports(&self) -> NodePorts {
        NodePorts::new().add_input_port(None).add_output_port(None)
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _ctx: NodeCtx,
    ) -> Result<Box<dyn DagNode>, dag_core::registry::error::Error> {
        let s: NormalizeRowsSpec = serde_json::from_value(spec)?;
        Ok(Box::new(NormalizeRowsNode {
            columns: s.columns,
            norm: s.norm,
            meta: self.ports(),
        }))
    }
}

#[derive(Clone)]
struct NormalizeRowsNode {
    columns: Vec<String>,
    norm: String,
    meta: NodePorts,
}

#[async_trait]
impl DagNode for NormalizeRowsNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }
    fn kind(&self) -> &'static str {
        "ml_normalize_rows"
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
        let mut data = extract_features(&batches, &self.columns)?;
        let kind = match self.norm.to_lowercase().as_str() {
            "l1" => NormKind::L1,
            "l2" => NormKind::L2,
            "max" => NormKind::Max,
            _ => NormKind::L2,
        };
        normalize_rows(&mut data, kind);
        let batch = replace_columns(&batches, &self.columns, &data, &[])?;
        emit_batch(ctx, batch)
    }
}
