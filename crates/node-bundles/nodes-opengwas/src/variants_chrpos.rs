//! `source_opengwas_variants_chrpos` — variant annotations by chr:pos from
//! OpenGWAS `/variants/chrpos`.

use async_trait::async_trait;
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;

use dag_core::codegen;
use dag_core::dag::{DagError, graph::PortOutputs};
use dag_core::node::{DagNode, NodePorts};
use dag_core::registry::{NodeCtx, NodeFactory};

use opengwas::types::VariantsChrposRequest;

use crate::shared::{json_to_output, make_client, single_output_port};

const KIND_VARIANTS_CHRPOS: &str = "source_opengwas_variants_chrpos";

/// Spec for [`OpengwasVariantsChrposNode`].
#[derive(Debug, Clone, JsonSchema, Deserialize)]
pub struct OpengwasVariantsChrposSpec {
    /// chr:pos strings on hg19/b37, e.g. `["7:105561135", "10:44865737"]`.
    pub chrpos: Vec<String>,
    /// Search radius in bp around each locus. Default `0`.
    #[serde(default)]
    pub radius: Option<i32>,
}

/// Source node: variant annotations by chr:pos from OpenGWAS `/variants/chrpos`.
#[derive(Clone)]
pub struct OpengwasVariantsChrposNode {
    meta: NodePorts,
    spec: OpengwasVariantsChrposSpec,
}

pub struct OpengwasVariantsChrposNodeFactory;

impl NodeFactory for OpengwasVariantsChrposNodeFactory {
    fn kind(&self) -> &'static str {
        KIND_VARIANTS_CHRPOS
    }
    fn desc(&self) -> &'static str {
        "Fetches variant annotations by chr:pos from OpenGWAS /variants/chrpos."
    }
    fn doc(&self) -> &'static str {
        "A source node that queries the OpenGWAS `/variants/chrpos` endpoint for \
        variant annotations by chromosome:position (hg19/b37), optionally with a \
        search radius.\n\n\
        Output schema is inferred — typically `name, chr, position, ref, alt`."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(OpengwasVariantsChrposSpec)
    }
    fn ports(&self) -> NodePorts {
        single_output_port()
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let s: OpengwasVariantsChrposSpec = serde_json::from_value(spec)?;
        Ok(Box::new(OpengwasVariantsChrposNode {
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
        let s = parse_spec::<OpengwasVariantsChrposSpec>(spec, KIND_VARIANTS_CHRPOS)?;
        let out = ctx.output_var.to_string();
        let chrpos = r_vec(&s.chrpos);
        let radius = s.radius.unwrap_or(0);
        let code = vec![
            format!("# OpenGWAS variant annotations by chr:pos (radius={radius}bp)"),
            format!("# NOTE: requires ieugwasr and OPENGWAS_TOKEN"),
            format!("{out} <- ieugwasr::variants_chrpos(c({chrpos}), radius = {radius})"),
        ];
        Ok(codegen::NodeCodegen::simple(code, out))
    }
    fn r_packages(&self) -> Vec<String> {
        vec!["ieugwasr".into()]
    }
}

#[async_trait]
impl DagNode for OpengwasVariantsChrposNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new((*self).clone())
    }
    fn kind(&self) -> &'static str {
        KIND_VARIANTS_CHRPOS
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
            .variants_chrpos(&VariantsChrposRequest {
                chrpos: self.spec.chrpos.clone(),
                radius: self.spec.radius,
            })
            .await
            .map_err(|e| DagError::Schedule(format!("OpenGWAS /variants/chrpos failed: {e}")))?;
        json_to_output(node_ctx, &resp, "/variants/chrpos").await
    }
}
