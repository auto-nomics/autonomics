use super::*;

// ═══════════════════════════════════════════════════════════════════════
// t-SNE
// ═══════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct TsneSpec {
    pub features: Vec<String>,
    #[serde(default = "d_tsne_dims")]
    pub n_components: usize,
    #[serde(default = "d_tsne_perplexity")]
    pub perplexity: f64,
    #[serde(default = "d_tsne_iter")]
    pub max_iter: usize,
    #[serde(default = "d_tsne_threshold")]
    pub approx_threshold: f64,
}
fn d_tsne_dims() -> usize {
    2
}
fn d_tsne_perplexity() -> f64 {
    5.0
}
fn d_tsne_iter() -> usize {
    1000
}
fn d_tsne_threshold() -> f64 {
    350.0
}

pub struct TsneFactory;
impl NodeFactory for TsneFactory {
    fn kind(&self) -> &'static str {
        "ml_tsne"
    }
    fn desc(&self) -> &'static str {
        "t-SNE non-linear dimensionality reduction."
    }
    fn doc(&self) -> &'static str {
        "t-SNE: maps high-dimensional data to 2D/3D for visualisation via Barnes-Hut t-SNE. Outputs tsne_0, tsne_1 columns."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(TsneSpec)
    }
    fn ports(&self) -> NodePorts {
        NodePorts::new().add_input_port(None).add_output_port(None)
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _ctx: NodeCtx,
    ) -> Result<Box<dyn DagNode>, dag_core::registry::error::Error> {
        let s: TsneSpec = serde_json::from_value(spec)?;
        Ok(Box::new(TsneNode {
            features: s.features,
            n_components: s.n_components,
            perplexity: s.perplexity,
            max_iter: s.max_iter,
            approx_threshold: s.approx_threshold,
            meta: self.ports(),
        }))
    }
}

#[derive(Clone)]
struct TsneNode {
    features: Vec<String>,
    n_components: usize,
    perplexity: f64,
    max_iter: usize,
    approx_threshold: f64,
    meta: NodePorts,
}

#[async_trait]
impl DagNode for TsneNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }
    fn kind(&self) -> &'static str {
        "ml_tsne"
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
                node_type: "ml_tsne".into(),
                msg: e.to_string(),
            })?;
        let opts = ml::dimred::TsneOptions {
            embedding_size: self.n_components,
            perplexity: self.perplexity,
            max_iter: self.max_iter,
            approx_threshold: self.approx_threshold,
            ..Default::default()
        };
        let model = ml::dimred::tsne(&data, &opts).map_err(|e| DagError::NodeError {
            node_type: "ml_tsne".into(),
            msg: e.to_string(),
        })?;
        let batch = build_embedding_output(&batches, &model.embedding, "tsne")?;
        emit_batch(ctx, batch)
    }
}
