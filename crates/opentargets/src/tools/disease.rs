use std::sync::Arc;

use agentik_core::tools::{ToolError, ToolFunction};
use agentik_proc::tool;
use agentik_sdk::types::ToolResult as AgentToolResult;
use async_trait::async_trait;

use super::json_err;
use crate::format::format_disease;
use crate::OpenTargetsClient;

#[tool(
    name = "opentargets_disease",
    description = "Get the annotation card for a disease / phenotype by ontology ID \
                  (EFO, MONDO, HP, Orphanet). Returns name, description, therapeutic-area \
                  flag, and parent terms. Use opentargets_search first to resolve a \
                  disease name into its ontology ID."
)]
pub struct DiseaseInput {
    #[desc = "Ontology ID, e.g. 'MONDO_0004975' (Alzheimer disease), 'EFO_0000274' (asthma)."]
    pub efo_id: String,
}

pub struct DiseaseTool {
    pub(crate) client: Arc<OpenTargetsClient>,
}

#[async_trait]
impl ToolFunction for DiseaseTool {
    type Input = DiseaseInput;

    async fn run(&self, input: Self::Input) -> Result<AgentToolResult, ToolError> {
        let disease = self.client.disease(&input.efo_id).await.map_err(json_err)?;
        match disease {
            Some(d) => Ok(AgentToolResult::success(format_disease(&d))),
            None => Ok(AgentToolResult::error(format!(
                "No disease found for ID '{}'.",
                input.efo_id
            ))),
        }
    }
}
