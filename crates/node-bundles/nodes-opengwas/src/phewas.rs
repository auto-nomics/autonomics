//! `source_opengwas_phewas` — PheWAS hits across all GWAS datasets from
//! OpenGWAS `/phewas`.

use async_trait::async_trait;
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;

use dag_core::dag::{DagError, graph::PortOutputs};
use dag_core::node::{DagNode, NodePorts};
use dag_core::registry::{NodeCtx, NodeFactory};

use opengwas::types::PhewasRequest;

use crate::shared::{json_to_output, make_client, single_output_port};

const KIND_PHEWAS: &str = "source_opengwas_phewas";

fn default_phewas_pval() -> f64 {
    0.01
}

/// Spec for [`OpengwasPhewasNode`].
#[derive(Debug, Clone, JsonSchema, Deserialize)]
pub struct OpengwasPhewasSpec {
    /// Variant identifiers (rsID, chr:pos, or chr:pos range on hg19/b37).
    pub variant: Vec<String>,
    /// P-value threshold (must ≤ 0.01). Default `0.01`.
    #[serde(default = "default_phewas_pval")]
    pub pval: f64,
    /// Restrict to specific study indexes. If empty, searches all.
    #[serde(default)]
    pub index_list: Option<Vec<String>>,
}

/// Source node: PheWAS results from OpenGWAS `/phewas`.
#[derive(Clone)]
pub struct OpengwasPhewasNode {
    meta: NodePorts,
    spec: OpengwasPhewasSpec,
}

pub struct OpengwasPhewasNodeFactory;

impl NodeFactory for OpengwasPhewasNodeFactory {
    fn kind(&self) -> &'static str {
        KIND_PHEWAS
    }
    fn desc(&self) -> &'static str {
        "Performs PheWAS across all GWAS datasets via OpenGWAS /phewas."
    }
    fn doc(&self) -> &'static str {
        "A source node that queries the OpenGWAS `/phewas` endpoint, scanning \
        the specified variants across all available GWAS datasets and returning \
        significant associations (p ≤ threshold).\n\n\
        Output schema is inferred — typically `id, rsid, chr, position, ea, nea, \
        beta, se, pval, n, trait`."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(OpengwasPhewasSpec)
    }
    fn ports(&self) -> NodePorts {
        single_output_port()
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let s: OpengwasPhewasSpec = serde_json::from_value(spec)?;
        Ok(Box::new(OpengwasPhewasNode {
            meta: single_output_port(),
            spec: s,
        }))
    }
}

#[async_trait]
impl DagNode for OpengwasPhewasNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new((*self).clone())
    }
    fn kind(&self) -> &'static str {
        KIND_PHEWAS
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    async fn execute(
        &mut self,
        node_ctx: &NodeCtx,
        _inputs: &[dag_core::dag::NodeInput],
        _reporter: &dag_core::dag::node_event::NodeReporter,
    ) -> Result<PortOutputs, DagError> {
        let client = make_client()?;
        let resp = client
            .phewas(&PhewasRequest {
                variant: self.spec.variant.clone(),
                pval: Some(self.spec.pval),
                index_list: self.spec.index_list.clone(),
                commercial_approval_received: None,
            })
            .await
            .map_err(|e| DagError::Schedule(format!("OpenGWAS /phewas failed: {e}")))?;
        json_to_output(node_ctx, &resp, "/phewas").await
    }
}
