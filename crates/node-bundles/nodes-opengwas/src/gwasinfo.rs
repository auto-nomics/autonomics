//! `source_opengwas_gwasinfo` — study metadata by ID from OpenGWAS `/gwasinfo`.

use async_trait::async_trait;
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;

use dag_core::dag::{DagError, graph::PortOutputs};
use dag_core::node::{DagNode, NodePorts};
use dag_core::registry::{NodeCtx, NodeFactory};

use opengwas::types::GwasInfoRequest;

use crate::shared::{gwasinfo_to_output, make_client, single_output_port};

const KIND_GWASINFO: &str = "source_opengwas_gwasinfo";

/// Spec for [`OpengwasGwasinfoNode`].
#[derive(Debug, Clone, JsonSchema, Deserialize)]
pub struct OpengwasGwasinfoSpec {
    /// GWAS study IDs to look up, e.g. `["ieu-a-2", "ukb-b-19953"]`.
    pub id: Vec<String>,
}

/// Source node: study metadata from OpenGWAS `/gwasinfo`.
#[derive(Clone)]
pub struct OpengwasGwasinfoNode {
    meta: NodePorts,
    spec: OpengwasGwasinfoSpec,
}

pub struct OpengwasGwasinfoNodeFactory;

impl NodeFactory for OpengwasGwasinfoNodeFactory {
    fn kind(&self) -> &'static str {
        KIND_GWASINFO
    }
    fn desc(&self) -> &'static str {
        "Fetches GWAS study metadata by ID from OpenGWAS /gwasinfo."
    }
    fn doc(&self) -> &'static str {
        "A source node that fetches metadata for specific GWAS datasets by ID \
        and emits them as a DataFrame.\n\n\
        Output schema is the full `GwasInfo` record (id, trait, year, nsnp, \
        sample_size, ncase, ncontrol, sd, build, etc.)."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(OpengwasGwasinfoSpec)
    }
    fn ports(&self) -> NodePorts {
        single_output_port()
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let s: OpengwasGwasinfoSpec = serde_json::from_value(spec)?;
        Ok(Box::new(OpengwasGwasinfoNode {
            meta: single_output_port(),
            spec: s,
        }))
    }
}

#[async_trait]
impl DagNode for OpengwasGwasinfoNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new((*self).clone())
    }
    fn kind(&self) -> &'static str {
        KIND_GWASINFO
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
        let rows = client
            .gwasinfo(&GwasInfoRequest {
                id: self.spec.id.clone(),
            })
            .await
            .map_err(|e| DagError::Schedule(format!("OpenGWAS /gwasinfo failed: {e}")))?;
        gwasinfo_to_output(node_ctx, rows, "/gwasinfo").await
    }
}
