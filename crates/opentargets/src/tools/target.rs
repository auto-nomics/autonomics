use std::sync::Arc;

use agentik_core::tools::{ToolError, ToolFunction};
use agentik_proc::tool;
use agentik_sdk::types::ToolResult as AgentToolResult;
use async_trait::async_trait;

use super::json_err;
use crate::OpenTargetsClient;
use crate::format::format_target;

#[tool(
    name = "opentargets_target",
    description = "Get the annotation card for a gene / target by Ensembl ID. Returns \
                  approved symbol & name, biotype, genomic location, function \
                  descriptions, and essentiality. If you only have a gene symbol, use \
                  opentargets_search first to resolve the Ensembl ID (ENSG...)."
)]
pub struct TargetInput {
    #[desc = "Ensembl gene ID, e.g. 'ENSG00000012048'."]
    pub ensembl_id: String,
}

pub struct TargetTool {
    pub(crate) client: Arc<OpenTargetsClient>,
}

#[async_trait]
impl ToolFunction for TargetTool {
    type Input = TargetInput;

    async fn run(&self, input: Self::Input) -> Result<AgentToolResult, ToolError> {
        let target = self
            .client
            .target(&input.ensembl_id)
            .await
            .map_err(json_err)?;
        match target {
            Some(t) => Ok(AgentToolResult::success(format_target(&t))),
            None => Ok(AgentToolResult::error(format!(
                "No target found for Ensembl ID '{}'.",
                input.ensembl_id
            ))),
        }
    }
}
