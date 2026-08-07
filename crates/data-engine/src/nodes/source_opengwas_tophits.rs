//! OpenGWAS tophits source node.
//!
//! [`OpengwasTophitsNode`] (`source_opengwas_tophits`) calls the OpenGWAS
//! `POST /tophits` endpoint and emits the top-associated SNPs as a
//! DataFusion `DataFrame`. The schema is inferred dynamically from the
//! JSON response.
//!
//! This is a zero-input / single-output source node. It requires the
//! `OPENGWAS_TOKEN` environment variable.

use async_trait::async_trait;
use datafusion::common::HashMap;
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;

use crate::dag::{DagError, graph::PortOutputs};
use crate::node_registry::registry::{NodeCtx, NodeFactory};
use crate::nodes::meta::{DagNode, NodePorts};
use crate::nodes::source_opengwas::{build_json_batch, extract_rows, make_client, single_output_port};

// ---------------------------------------------------------------------------
// Spec
// ---------------------------------------------------------------------------

fn default_pval() -> f64 {
    5e-8
}
fn default_clump() -> i32 {
    1
}
fn default_r2() -> f64 {
    0.001
}
fn default_kb() -> i32 {
    5000
}
fn default_pop() -> String {
    "EUR".to_string()
}

/// Spec for [`OpengwasTophitsNode`].
#[derive(Debug, Clone, JsonSchema, Deserialize)]
pub struct OpengwasTophitsSpec {
    /// GWAS study IDs to query, e.g. `["ukb-b-19953"]`.
    pub id: Vec<String>,
    /// P-value threshold (must be ≤ 0.01). Default `5e-8`.
    #[serde(default = "default_pval")]
    pub pval: f64,
    /// Whether to clump results server-side: `1` (yes) or `0` (no). Default `1`.
    #[serde(default = "default_clump")]
    pub clump: i32,
    /// Clumping r² threshold. Default `0.001`.
    #[serde(default = "default_r2")]
    pub r2: f64,
    /// Clumping window size in kb. Default `5000`.
    #[serde(default = "default_kb")]
    pub kb: i32,
    /// Reference population for clumping. Default `"EUR"`.
    #[serde(default = "default_pop")]
    pub pop: String,
}

// ---------------------------------------------------------------------------
// Node + Factory
// ---------------------------------------------------------------------------

const SOURCE_OPENGWAS_TOPHITS_KIND: &str = "source_opengwas_tophits";

/// Source node that fetches top GWAS hits from OpenGWAS `/tophits`.
#[derive(Clone)]
pub struct OpengwasTophitsNode {
    meta: NodePorts,
    spec: OpengwasTophitsSpec,
}

impl OpengwasTophitsNode {
    pub fn new(spec: OpengwasTophitsSpec) -> Self {
        Self {
            meta: single_output_port(),
            spec,
        }
    }
}

pub struct OpengwasTophitsNodeFactory {}

impl NodeFactory for OpengwasTophitsNodeFactory {
    fn kind(&self) -> &'static str {
        SOURCE_OPENGWAS_TOPHITS_KIND
    }

    fn desc(&self) -> &'static str {
        "Fetches top GWAS hits from the OpenGWAS /tophits endpoint as a table."
    }

    fn doc(&self) -> &'static str {
        "A source node that queries the OpenGWAS `/tophits` endpoint for \
        top-associated SNPs (optionally clumped) and emits them as a \
        DataFrame. No input ports; one output port.\n\n\
        Requires the `OPENGWAS_TOKEN` environment variable.\n\n\
        The output schema is inferred from the API response — typically \
        `rsid`, `chr`, `position`, `ea`, `nea`, `eaf`, `beta`, `se`, `pval`, \
        `samplesize`, `ncontrol`, `ncase`, etc."
    }

    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(OpengwasTophitsSpec)
    }

    fn ports(&self) -> NodePorts {
        single_output_port()
    }

    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> crate::node_registry::error::Result<Box<dyn DagNode>> {
        let node_spec: OpengwasTophitsSpec = serde_json::from_value(spec)?;
        Ok(Box::new(OpengwasTophitsNode::new(node_spec)))
    }

    fn codegen_r(
        &self,
        spec: &serde_json::Value,
        ctx: &mut crate::codegen::CodegenCtx,
    ) -> std::result::Result<crate::codegen::NodeCodegen, crate::codegen::CodegenError> {
        use crate::codegen::helpers::*;
        let s = parse_spec::<OpengwasTophitsSpec>(spec, SOURCE_OPENGWAS_TOPHITS_KIND)?;
        let out = ctx.output_var.to_string();
        let ids = s.id.join("\", \"");
        let code = vec![
            format!("# OpenGWAS tophits for: [\"{}\"]", ids),
            format!("# NOTE: requires the TwoSampleMR R package and OPENGWAS_TOKEN"),
            format!("ao <- extract_outcome_data(snps = c(), outcomes = c(\"{}\"))", ids),
            format!(
                "# Top hits with pval ≤ {} (clump={}, pop=\"{}\")",
                s.pval, s.clump, s.pop
            ),
            format!("{out} <- ao[ao$pval.outcome <= {}, ]", s.pval),
        ];
        Ok(crate::codegen::NodeCodegen::simple(code, out))
    }

    fn r_packages(&self) -> Vec<String> {
        vec!["TwoSampleMR".into()]
    }
}

#[async_trait]
impl DagNode for OpengwasTophitsNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }

    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new((*self).clone())
    }

    fn kind(&self) -> &'static str {
        SOURCE_OPENGWAS_TOPHITS_KIND
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    async fn execute(
        &mut self,
        node_ctx: &NodeCtx,
        _inputs: &[crate::dag::NodeInput],
        _reporter: &crate::dag::node_event::NodeReporter,
    ) -> Result<PortOutputs, DagError> {
        let client = make_client()?;

        let req = opengwas::types::TophitsRequest {
            id: self.spec.id.clone(),
            pval: Some(self.spec.pval),
            preclumped: None,
            clump: Some(self.spec.clump),
            r2: Some(self.spec.r2),
            kb: Some(self.spec.kb),
            pop: Some(self.spec.pop.clone()),
            commercial_approval_received: None,
        };

        let resp = client
            .tophits(&req)
            .await
            .map_err(|e| DagError::Schedule(format!("OpenGWAS /tophits request failed: {e}")))?;

        let rows = extract_rows(&resp);
        tracing::info!(
            "OpenGWAS tophits: {} SNPs returned for {} (pval={}, clump={})",
            rows.len(),
            self.spec.id.join(", "),
            self.spec.pval,
            self.spec.clump,
        );

        let batch = build_json_batch(&rows)?;
        let session = node_ctx.session();
        let df = session
            .read_batch(batch)
            .map_err(|e| DagError::Schedule(format!("failed to read tophits batch: {e}")))?;

        let mut res: PortOutputs = HashMap::new();
        res.insert(0, df);
        Ok(res)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_deserialises() {
        let json = serde_json::json!({
            "id": ["ukb-b-19953"],
        });
        let spec: OpengwasTophitsSpec = serde_json::from_value(json).unwrap();
        assert_eq!(spec.id, vec!["ukb-b-19953"]);
        assert!((spec.pval - 5e-8).abs() < f64::EPSILON);
        assert_eq!(spec.clump, 1);
        assert_eq!(spec.pop, "EUR");
    }

    #[test]
    fn spec_with_overrides() {
        let json = serde_json::json!({
            "id": ["ieu-a-2", "ukb-b-1"],
            "pval": 1e-5,
            "clump": 0,
            "r2": 0.01,
            "kb": 1000,
            "pop": "EAS",
        });
        let spec: OpengwasTophitsSpec = serde_json::from_value(json).unwrap();
        assert_eq!(spec.id.len(), 2);
        assert!((spec.pval - 1e-5).abs() < f64::EPSILON);
        assert_eq!(spec.clump, 0);
        assert!((spec.r2 - 0.01).abs() < f64::EPSILON);
        assert_eq!(spec.kb, 1000);
        assert_eq!(spec.pop, "EAS");
    }
}
