use super::*;

// ═══════════════════════════════════════════════════════════════════════
// FastICA
// ═══════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct IcaSpec {
    pub features: Vec<String>,
    pub n_components: usize,
}

pub struct IcaFactory;
impl NodeFactory for IcaFactory {
    fn kind(&self) -> &'static str {
        "ml_ica"
    }
    fn desc(&self) -> &'static str {
        "Independent Component Analysis (FastICA)."
    }
    fn doc(&self) -> &'static str {
        "FastICA: separates multivariate signal into additive independent components. Outputs ic_0, ic_1, … appended to input table."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(IcaSpec)
    }
    fn ports(&self) -> NodePorts {
        NodePorts::new().add_input_port(None).add_output_port(None)
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _ctx: NodeCtx,
    ) -> Result<Box<dyn DagNode>, dag_core::registry::error::Error> {
        let s: IcaSpec = serde_json::from_value(spec)?;
        Ok(Box::new(IcaNode {
            features: s.features,
            n_components: s.n_components,
            meta: self.ports(),
        }))
    }
}

#[derive(Clone)]
struct IcaNode {
    features: Vec<String>,
    n_components: usize,
    meta: NodePorts,
}

#[async_trait]
impl DagNode for IcaNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }
    fn kind(&self) -> &'static str {
        "ml_ica"
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
        let data =
            common::extract_matrix(&batches, &self.features).map_err(|e| DagError::NodeError {
                node_type: "ml_ica".into(),
                msg: e.to_string(),
            })?;
        let model =
            ml::dimred::fast_ica(&data, self.n_components).map_err(|e| DagError::NodeError {
                node_type: "ml_ica".into(),
                msg: e.to_string(),
            })?;
        let batch = build_embedding_output(&batches, &model.components, "ic")?;
        emit_batch(ctx, batch)
    }
}
