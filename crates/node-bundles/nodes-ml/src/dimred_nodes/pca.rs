use super::*;

// ═══════════════════════════════════════════════════════════════════════
// PCA
// ═══════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct PcaSpec {
    pub features: Vec<String>,
    pub n_components: usize,
}

pub struct PcaFactory;
impl NodeFactory for PcaFactory {
    fn kind(&self) -> &'static str {
        "ml_pca"
    }
    fn desc(&self) -> &'static str {
        "Principal Component Analysis (PCA)."
    }
    fn doc(&self) -> &'static str {
        "PCA: linear dimensionality reduction via SVD of the covariance matrix. Outputs projected data (pc_0, pc_1, …) appended to the input table."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(PcaSpec)
    }
    fn ports(&self) -> NodePorts {
        NodePorts::new().add_input_port(None).add_output_port(None)
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _ctx: NodeCtx,
    ) -> Result<Box<dyn DagNode>, dag_core::registry::error::Error> {
        let s: PcaSpec = serde_json::from_value(spec)?;
        Ok(Box::new(PcaNode {
            features: s.features,
            n_components: s.n_components,
            meta: self.ports(),
        }))
    }
}

#[derive(Clone)]
struct PcaNode {
    features: Vec<String>,
    n_components: usize,
    meta: NodePorts,
}

#[async_trait]
impl DagNode for PcaNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }
    fn kind(&self) -> &'static str {
        "ml_pca"
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
                node_type: "ml_pca".into(),
                msg: e.to_string(),
            })?;
        let model = ml::dimred::pca(&data, self.n_components).map_err(|e| DagError::NodeError {
            node_type: "ml_pca".into(),
            msg: e.to_string(),
        })?;
        let transformed = ml::dimred::pca_transform(&model, &data);
        let (nrows, ncols) = transformed.shape();
        let embedding: Vec<Vec<f64>> = (0..nrows)
            .map(|i| (0..ncols).map(|j| transformed[(i, j)]).collect())
            .collect();
        let batch = build_embedding_output(&batches, &embedding, "pc")?;
        emit_batch(ctx, batch)
    }
}
