use super::*;

// ═══════════════════════════════════════════════════════════════════════
// Truncated SVD / LSA
// ═══════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct TruncatedSvdSpec {
    pub features: Vec<String>,
    pub n_components: usize,
}

pub struct TruncatedSvdFactory;
impl NodeFactory for TruncatedSvdFactory {
    fn kind(&self) -> &'static str {
        "ml_truncated_svd"
    }
    fn desc(&self) -> &'static str {
        "Truncated SVD (Latent Semantic Analysis)."
    }
    fn doc(&self) -> &'static str {
        "TruncatedSVD: dimensionality reduction via thin SVD without centering. Suitable for sparse text/count data. Outputs lsa_0, lsa_1, … columns."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(TruncatedSvdSpec)
    }
    fn ports(&self) -> NodePorts {
        NodePorts::new().add_input_port(None).add_output_port(None)
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _ctx: NodeCtx,
    ) -> Result<Box<dyn DagNode>, dag_core::registry::error::Error> {
        let s: TruncatedSvdSpec = serde_json::from_value(spec)?;
        Ok(Box::new(TruncatedSvdNode {
            features: s.features,
            n_components: s.n_components,
            meta: self.ports(),
        }))
    }
}

#[derive(Clone)]
struct TruncatedSvdNode {
    features: Vec<String>,
    n_components: usize,
    meta: NodePorts,
}

#[async_trait]
impl DagNode for TruncatedSvdNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }
    fn kind(&self) -> &'static str {
        "ml_truncated_svd"
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
                node_type: "ml_truncated_svd".into(),
                msg: e.to_string(),
            })?;
        let model = ml::dimred::truncated_svd(&data, self.n_components).map_err(|e| {
            DagError::NodeError {
                node_type: "ml_truncated_svd".into(),
                msg: e.to_string(),
            }
        })?;
        // Project: X @ V_k (components^T) → n × n_components
        let (nrows, ncols) = data.shape();
        let embedding: Vec<Vec<f64>> = (0..nrows)
            .map(|i| {
                (0..model.n_components)
                    .map(|k| {
                        let mut val = 0.0;
                        for j in 0..ncols {
                            val += data[(i, j)] * model.components[k][j];
                        }
                        val
                    })
                    .collect()
            })
            .collect();
        let batch = build_embedding_output(&batches, &embedding, "lsa")?;
        emit_batch(ctx, batch)
    }
}
