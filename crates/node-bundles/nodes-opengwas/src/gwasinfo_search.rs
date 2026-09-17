//! `source_opengwas_gwasinfo_search` — SQL LIKE search across cached GWAS
//! metadata.

use async_trait::async_trait;
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;

use dag_core::dag::{DagError, graph::PortOutputs};
use dag_core::node::{DagNode, NodePorts};
use dag_core::registry::{NodeCtx, NodeFactory};

use crate::shared::{gwasinfo_to_output, make_client, single_output_port};

const KIND_GWASINFO_SEARCH: &str = "source_opengwas_gwasinfo_search";

fn default_search_field() -> String {
    "trait".to_string()
}
fn default_search_limit() -> i64 {
    50
}

/// Spec for [`OpengwasGwasinfoSearchNode`].
#[derive(Debug, Clone, JsonSchema, Deserialize)]
pub struct OpengwasGwasinfoSearchSpec {
    /// Search keyword (case-insensitive substring match).
    pub keyword: String,
    /// Column to search: `"trait"` (default), `"author"`, or `"population"`.
    #[serde(default = "default_search_field")]
    pub field: String,
    /// Maximum results to return. Default `50`.
    #[serde(default = "default_search_limit")]
    pub limit: i64,
    /// Column to sort by: `nsnp`, `sample_size`, `year`, `ncase`, etc.
    #[serde(default)]
    pub sort_by: Option<String>,
    /// Sort order: `"desc"` (default) or `"asc"`.
    #[serde(default)]
    pub sort_order: Option<String>,
}

/// Source node: keyword search across cached GWAS datasets.
#[derive(Clone)]
pub struct OpengwasGwasinfoSearchNode {
    meta: NodePorts,
    spec: OpengwasGwasinfoSearchSpec,
}

pub struct OpengwasGwasinfoSearchNodeFactory;

impl NodeFactory for OpengwasGwasinfoSearchNodeFactory {
    fn kind(&self) -> &'static str {
        KIND_GWASINFO_SEARCH
    }
    fn desc(&self) -> &'static str {
        "Searches cached GWAS datasets by keyword via OpenGWAS."
    }
    fn doc(&self) -> &'static str {
        "A source node that searches the OpenGWAS cached metadata by keyword \
        (SQL LIKE on an indexed column) and emits matching datasets as a \
        DataFrame.\n\n\
        Output schema is the same as `source_opengwas_gwasinfo`."
    }
    fn spec_schema(&self) -> schemars::Schema {
        schema_for!(OpengwasGwasinfoSearchSpec)
    }
    fn ports(&self) -> NodePorts {
        single_output_port()
    }
    fn build(
        &self,
        spec: serde_json::Value,
        _node_ctx: NodeCtx,
    ) -> dag_core::registry::error::Result<Box<dyn DagNode>> {
        let s: OpengwasGwasinfoSearchSpec = serde_json::from_value(spec)?;
        Ok(Box::new(OpengwasGwasinfoSearchNode {
            meta: single_output_port(),
            spec: s,
        }))
    }
}

#[async_trait]
impl DagNode for OpengwasGwasinfoSearchNode {
    fn ports(&self) -> &NodePorts {
        &self.meta
    }
    fn clone_box(&self) -> Box<dyn DagNode> {
        Box::new((*self).clone())
    }
    fn kind(&self) -> &'static str {
        KIND_GWASINFO_SEARCH
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
        // Map external "trait" → internal column "trait_".
        let db_field = if self.spec.field == "trait" {
            "trait_"
        } else {
            &self.spec.field
        };
        let sort_by = self
            .spec
            .sort_by
            .as_deref()
            .map(|s| if s == "trait" { "trait_" } else { s });
        let rows = client
            .gwasinfo_search(
                &self.spec.keyword,
                db_field,
                self.spec.limit,
                sort_by,
                self.spec.sort_order.as_deref(),
            )
            .await
            .map_err(|e| DagError::Schedule(format!("OpenGWAS gwasinfo_search failed: {e}")))?;
        gwasinfo_to_output(node_ctx, rows, "gwasinfo_search").await
    }
}
