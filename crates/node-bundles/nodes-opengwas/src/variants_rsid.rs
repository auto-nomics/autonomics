//! `source_opengwas_variants_rsid` — variant annotations by rsID from
//! OpenGWAS `/variants/rsid`.

use async_trait::async_trait;
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;

use dag_core::codegen;
use dag_core::dag::{DagError, graph::PortOutputs};
use dag_core::node::{DagNode, NodePorts};
use dag_core::registry::{NodeCtx, NodeFactory};

use opengwas::types::VariantsRsidRequest;

use crate::shared::{json_to_output, make_client, single_output_port};

const KIND_VARIANTS_RSID: &str = "source_opengwas_variants_rsid";

/// Spec for [`OpengwasVariantsRsidNode`].
#[derive(Debug, Clone, JsonSchema, Deserialize)]
pub struct OpengwasVariantsRsidSpec {
    /// Variant rs IDs, e.g. `["rs1205", "rs234"]`.
    pub rsid: Vec<String>,
}

/// Source node: variant annotations by rsID from OpenGWAS `/variants/rsid`.
#[derive(Clone)]
pub struct OpengwasVariantsRsidNode {
    meta: NodePorts,
    spec: OpengwasVariantsRsidSpec,
}

pub struct OpengwasVariantsRsidNodeFactory;

impl NodeFactory for OpengwasVariantsRsidNodeFactory {
    fn kind(&self) -> &'static str {
        KIND_VARIANTS_RSID
    }
    fn desc(&self) -> &'static str {
        "Fetches variant annotations by rsID from OpenGWAS /variants/rsid."
    }
    fn doc(&self) -> &'static str {
        "A source node that queries the OpenGWAS `/variants/rsid` endpoint for \
        variant annotations (chromosome, position, alleles) by rsID.\n\n\
        Output schema is inferred — typically `name, chr, position, ref, alt`."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(OpengwasVariantsRsidSpec)
    }
    fn ports(&self) -> NodePorts {
        single_output_port()
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let s: OpengwasVariantsRsidSpec = serde_json::from_value(spec)?;
        Ok(Box::new(OpengwasVariantsRsidNode {
            meta: single_output_port(),
            spec: s,
        }))
    }
    fn codegen_r(
        &self,
        spec: &serde_json::Value,
        ctx: &mut codegen::CodegenCtx,
    ) -> std::result::Result<codegen::NodeCodegen, codegen::CodegenError> {
        use codegen::helpers::*;
        let s = parse_spec::<OpengwasVariantsRsidSpec>(spec, KIND_VARIANTS_RSID)?;
        let out = ctx.output_var.to_string();
        let rsids = r_vec(&s.rsid);
        let code = vec![
            format!("# OpenGWAS variant annotations by rsID"),
            format!("# NOTE: requires ieugwasr and OPENGWAS_TOKEN"),
            format!("{out} <- ieugwasr::variants_rsid(c({rsids}))"),
        ];
        Ok(codegen::NodeCodegen::simple(code, out))
    }
    fn r_packages(&self) -> Vec<String> {
        vec!["ieugwasr".into()]
    }
}

#[async_trait]
impl DagNode for OpengwasVariantsRsidNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new((*self).clone())
    }
    fn kind(&self) -> &'static str {
        KIND_VARIANTS_RSID
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
            .variants_rsid(&VariantsRsidRequest {
                rsid: self.spec.rsid.clone(),
            })
            .await
            .map_err(|e| DagError::Schedule(format!("OpenGWAS /variants/rsid failed: {e}")))?;
        json_to_output(node_ctx, &resp, "/variants/rsid").await
    }
}
