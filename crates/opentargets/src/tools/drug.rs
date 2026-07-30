use std::sync::Arc;

use agentik_core::tools::{ToolError, ToolFunction};
use agentik_proc::tool;
use agentik_sdk::types::ToolResult as AgentToolResult;
use async_trait::async_trait;

use super::json_err;
use crate::OpenTargetsClient;
use crate::format::format_drug;

#[tool(
    name = "opentargets_drug",
    description = "Get the annotation card for a drug / compound by ChEMBL ID. Returns \
                  name, drug type, maximum clinical stage, and description. Use \
                  opentargets_search first to resolve a drug name into its ChEMBL ID \
                  (CHEMBL...)."
)]
pub struct DrugInput {
    #[desc = "ChEMBL ID, e.g. 'CHEMBL25' (aspirin)."]
    pub chembl_id: String,
}

pub struct DrugTool {
    pub(crate) client: Arc<OpenTargetsClient>,
}

#[async_trait]
impl ToolFunction for DrugTool {
    type Input = DrugInput;

    async fn run(&self, input: Self::Input) -> Result<AgentToolResult, ToolError> {
        let drug = self.client.drug(&input.chembl_id).await.map_err(json_err)?;
        match drug {
            Some(d) => Ok(AgentToolResult::success(format_drug(&d))),
            None => Ok(AgentToolResult::error(format!(
                "No drug found for ChEMBL ID '{}'.",
                input.chembl_id
            ))),
        }
    }
}
