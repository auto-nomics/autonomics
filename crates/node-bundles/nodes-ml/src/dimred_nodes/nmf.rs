use super::*;

// ═══════════════════════════════════════════════════════════════════════
// NMF
// ═══════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct NmfSpec {
    pub features: Vec<String>,
    pub n_components: usize,
    #[serde(default = "d_nmf_iter")]
    pub max_iter: usize,
    #[serde(default = "d_nmf_tol")]
    pub tol: f64,
    #[serde(default = "d_nmf_seed")]
    pub seed: u64,
}
fn d_nmf_iter() -> usize {
    200
}
fn d_nmf_tol() -> f64 {
    1e-4
}
fn d_nmf_seed() -> u64 {
    42
}

pub struct NmfFactory;
impl NodeFactory for NmfFactory {
    fn kind(&self) -> &'static str {
        "ml_nmf"
    }
    fn desc(&self) -> &'static str {
        "Non-negative Matrix Factorization (NMF)."
    }
    fn doc(&self) -> &'static str {
        "NMF: factorises a non-negative matrix V ≈ W·H. Outputs W (basis weights) as nmf_0, nmf_1, … columns."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(NmfSpec)
    }
    fn ports(&self) -> NodePorts {
        NodePorts::new().add_input_port(None).add_output_port(None)
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _ctx: NodeCtx,
    ) -> Result<Box<dyn DagNode>, dag_core::registry::error::Error> {
        let s: NmfSpec = serde_json::from_value(spec)?;
        Ok(Box::new(NmfNode {
            features: s.features,
            n_components: s.n_components,
            max_iter: s.max_iter,
            tol: s.tol,
            seed: s.seed,
            meta: self.ports(),
        }))
    }
}

#[derive(Clone)]
struct NmfNode {
    features: Vec<String>,
    n_components: usize,
    max_iter: usize,
    tol: f64,
    seed: u64,
    meta: NodePorts,
}

#[async_trait]
impl DagNode for NmfNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new(self.clone())
    }
    fn kind(&self) -> &'static str {
        "ml_nmf"
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
                node_type: "ml_nmf".into(),
                msg: e.to_string(),
            })?;
        let model = ml::dimred::nmf(&data, self.n_components, self.max_iter, self.tol, self.seed)
            .map_err(|e| DagError::NodeError {
            node_type: "ml_nmf".into(),
            msg: e.to_string(),
        })?;
        let batch = build_embedding_output(&batches, &model.w, "nmf")?;
        emit_batch(ctx, batch)
    }
}
