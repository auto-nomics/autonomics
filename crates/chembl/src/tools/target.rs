use std::sync::Arc;

use agentik_core::tools::{ToolError, ToolFunction};
use agentik_proc::tool;
use agentik_sdk::types::ToolResult as AgentToolResult;
use async_trait::async_trait;

use super::json_err;
use crate::{ChEMBLClient, format::format_target};

#[tool(
    name = "chembl_target_summary",
    description = "Get a biological target information card from ChEMBL by ChEMBL ID, \
                  including organism, target type, protein components, UniProt accessions, \
                  and gene symbols."
)]
pub struct TargetInput {
    #[desc = "ChEMBL target ID, e.g. 'CHEMBL2094253'."]
    pub chembl_id: String,
}

pub struct TargetTool {
    pub(crate) client: Arc<ChEMBLClient>,
}

#[async_trait]
impl ToolFunction for TargetTool {
    type Input = TargetInput;

    async fn run(&self, input: Self::Input) -> Result<AgentToolResult, ToolError> {
        match self
            .client
            .target(&input.chembl_id)
            .await
            .map_err(json_err)?
        {
            Some(target) => Ok(AgentToolResult::success(format_target(&target))),
            None => Ok(AgentToolResult::error(format!(
                "No ChEMBL target found for '{}'.",
                input.chembl_id
            ))),
        }
    }
}
