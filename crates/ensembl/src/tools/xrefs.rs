use std::sync::Arc;

use agentik_core::tools::{ToolError, ToolFunction};
use agentik_proc::tool;
use agentik_sdk::types::ToolResult as AgentToolResult;
use async_trait::async_trait;

use super::json_err;
use crate::EnsemblClient;
use crate::format::format_xrefs;

#[tool(
    name = "ensembl_xrefs",
    description = "Preview external database cross-references for an Ensembl stable ID, including HGNC, EntrezGene, RefSeq, UniProt, and other resources."
)]
pub struct EnsemblXrefsInput {
    #[desc = "Ensembl ID, e.g. 'ENSG00000139618'."]
    pub id: String,

    #[desc = "Return dependent xrefs for transcript and protein children. Default true."]
    pub all_levels: Option<bool>,

    #[desc = "Optional external database filter, e.g. 'HGNC', 'EntrezGene', or 'UniProtKB'."]
    pub external_db: Option<String>,

    #[desc = "Maximum xrefs shown (default 25)."]
    pub limit: Option<usize>,
}

pub struct EnsemblXrefsTool {
    pub(crate) client: Arc<EnsemblClient>,
}

#[async_trait]
impl ToolFunction for EnsemblXrefsTool {
    type Input = EnsemblXrefsInput;

    async fn run(&self, input: Self::Input) -> Result<AgentToolResult, ToolError> {
        let xrefs = self
            .client
            .xrefs(
                &input.id,
                input.all_levels.unwrap_or(true),
                input.external_db.as_deref(),
            )
            .await
            .map_err(json_err)?;
        let limit = input.limit.unwrap_or(25);
        Ok(AgentToolResult::success(format_xrefs(
            &xrefs,
            limit.min(xrefs.len()),
        )))
    }
}
