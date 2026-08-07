//! `source_opengwas_ld_clump` — LD-clumped independent loci from
//! OpenGWAS `/ld/clump`.

use async_trait::async_trait;
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;

use dag_core::codegen;
use dag_core::dag::{DagError, graph::PortOutputs};
use dag_core::node::{DagNode, NodePorts};
use dag_core::registry::{NodeCtx, NodeFactory};

use opengwas::types::LdClumpRequest;

use crate::shared::{json_to_output, make_client, single_output_port};

const KIND_LD_CLUMP: &str = "source_opengwas_ld_clump";

fn default_clump_r2() -> f64 {
    0.001
}
fn default_clump_kb() -> i32 {
    5000
}
fn default_clump_pop() -> String {
    "EUR".to_string()
}

/// Spec for [`OpengwasLdClumpNode`].
#[derive(Debug, Clone, JsonSchema, Deserialize)]
pub struct OpengwasLdClumpSpec {
    /// rs IDs to clump.
    pub rsid: Vec<String>,
    /// P-values for each SNP (same length as `rsid`).
    pub pval: Vec<f64>,
    /// Significance threshold. Default `5e-8`.
    #[serde(default)]
    pub pthresh: Option<f64>,
    /// LD r² threshold for clumping. Default `0.001`.
    #[serde(default = "default_clump_r2")]
    pub r2: f64,
    /// Clumping window size in kb. Default `5000`.
    #[serde(default = "default_clump_kb")]
    pub kb: i32,
    /// Reference population: `EUR`, `SAS`, `EAS`, `AFR`, `AMR`. Default `EUR`.
    #[serde(default = "default_clump_pop")]
    pub pop: String,
}

/// Source node: LD-clumped independent loci from OpenGWAS `/ld/clump`.
#[derive(Clone)]
pub struct OpengwasLdClumpNode {
    meta: NodePorts,
    spec: OpengwasLdClumpSpec,
}

pub struct OpengwasLdClumpNodeFactory;

impl NodeFactory for OpengwasLdClumpNodeFactory {
    fn kind(&self) -> &'static str {
        KIND_LD_CLUMP
    }
    fn desc(&self) -> &'static str {
        "Performs LD clumping via OpenGWAS /ld/clump and returns independent loci."
    }
    fn doc(&self) -> &'static str {
        "A source node that calls the OpenGWAS `/ld/clump` endpoint to clump a \
        set of SNPs (with associated p-values) into independent loci using 1000 \
        Genomes reference data.\n\n\
        Output schema is inferred — typically `rsid, chr, position, pval, n, clump, \
        total, prop, kp, kp_2`."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(OpengwasLdClumpSpec)
    }
    fn ports(&self) -> NodePorts {
        single_output_port()
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let s: OpengwasLdClumpSpec = serde_json::from_value(spec)?;
        Ok(Box::new(OpengwasLdClumpNode {
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
        let s = parse_spec::<OpengwasLdClumpSpec>(spec, KIND_LD_CLUMP)?;
        let out = ctx.output_var.to_string();
        let rsids = r_vec(&s.rsid);
        let pvals = r_vec_f64(&s.pval);
        let code = vec![
            format!("# OpenGWAS LD clumping (r2={}, kb={}, pop=\"{}\")", s.r2, s.kb, s.pop),
            format!("# NOTE: requires ieugwasr and OPENGWAS_TOKEN"),
            format!("_dat <- data.frame(rsid = c({rsids}), pval = c({pvals}))"),
            format!("{out} <- ieugwasr::ld_clump("),
            format!("  dat = _dat,"),
            format!("  clump_kb = {}, clump_r2 = {}, plink_bin = NULL", s.kb, s.r2),
            format!(")"),
        ];
        Ok(codegen::NodeCodegen::simple(code, out))
    }
    fn r_packages(&self) -> Vec<String> {
        vec!["ieugwasr".into()]
    }
}

#[async_trait]
impl DagNode for OpengwasLdClumpNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new((*self).clone())
    }
    fn kind(&self) -> &'static str {
        KIND_LD_CLUMP
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
            .ld_clump(&LdClumpRequest {
                rsid: self.spec.rsid.clone(),
                pval: self.spec.pval.clone(),
                pthresh: self.spec.pthresh,
                r2: Some(self.spec.r2),
                kb: Some(self.spec.kb),
                pop: Some(self.spec.pop.clone()),
            })
            .await
            .map_err(|e| DagError::Schedule(format!("OpenGWAS /ld/clump failed: {e}")))?;
        json_to_output(node_ctx, &resp, "/ld/clump").await
    }
}
