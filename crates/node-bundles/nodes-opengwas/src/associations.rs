//! `source_opengwas_associations` — variant × study associations from
//! OpenGWAS `/associations`.

use async_trait::async_trait;
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;

use dag_core::dag::{DagError, graph::PortOutputs};
use dag_core::node::{DagNode, NodePorts};
use dag_core::registry::{NodeCtx, NodeFactory};

use opengwas::types::AssociationsRequest;

use crate::shared::{json_to_output, make_client, single_output_port};

const KIND_ASSOC: &str = "source_opengwas_associations";

/// Spec for [`OpengwasAssociationsNode`].
#[derive(Debug, Clone, JsonSchema, Deserialize)]
pub struct OpengwasAssociationsSpec {
    /// Variants as rsID or chr:pos (hg19/b37), e.g. `["rs1205", "7:105561135"]`.
    pub variant: Vec<String>,
    /// GWAS study IDs, e.g. `["ieu-a-2", "ukb-b-19953"]`.
    pub id: Vec<String>,
    /// Look for proxies: `1` (yes) or `0` (no). Default `0`.
    #[serde(default)]
    pub proxies: Option<i32>,
    /// Reference population for proxies. Default `"EUR"`.
    #[serde(default)]
    pub population: Option<String>,
    /// Minimum LD r² for a proxy. Default `0.8`.
    #[serde(default)]
    pub r2: Option<f64>,
}

/// Source node: variant × study associations from OpenGWAS `/associations`.
#[derive(Clone)]
pub struct OpengwasAssociationsNode {
    meta: NodePorts,
    spec: OpengwasAssociationsSpec,
}

pub struct OpengwasAssociationsNodeFactory;

impl NodeFactory for OpengwasAssociationsNodeFactory {
    fn kind(&self) -> &'static str {
        KIND_ASSOC
    }
    fn desc(&self) -> &'static str {
        "Fetches variant–study associations from OpenGWAS /associations as a table."
    }
    fn doc(&self) -> &'static str {
        "A source node that queries the OpenGWAS `/associations` endpoint for \
        specific variant–study associations (beta, se, p-value, alleles) and \
        emits them as a DataFrame.\n\n\
        Output schema is inferred from the API response — typically \
        `rsid, chr, position, ea, nea, eaf, beta, se, pval`."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(OpengwasAssociationsSpec)
    }
    fn ports(&self) -> NodePorts {
        single_output_port()
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let s: OpengwasAssociationsSpec = serde_json::from_value(spec)?;
        Ok(Box::new(OpengwasAssociationsNode {
            meta: single_output_port(),
            spec: s,
        }))
    }
}

#[async_trait]
impl DagNode for OpengwasAssociationsNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new((*self).clone())
    }
    fn kind(&self) -> &'static str {
        KIND_ASSOC
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
            .associations(&AssociationsRequest {
                variant: self.spec.variant.clone(),
                id: self.spec.id.clone(),
                proxies: self.spec.proxies,
                population: self.spec.population.clone(),
                r2: self.spec.r2,
                align_alleles: None,
                palindromes: None,
                maf_threshold: None,
                commercial_approval_received: None,
            })
            .await
            .map_err(|e| DagError::Schedule(format!("OpenGWAS /associations failed: {e}")))?;
        json_to_output(node_ctx, &resp, "/associations").await
    }
}
