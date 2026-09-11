use std::sync::Arc;

use agentik_core::tools::{ToolError, ToolFunction};
use agentik_proc::tool;
use agentik_sdk::types::ToolResult as AgentToolResult;
use async_trait::async_trait;

use crate::InterProClient;
use crate::format::format_entry;

#[tool(
    name = "interpro_lookup",
    description = "Look up an InterPro entry by accession. Returns the entry name, type, \
                  member databases, GO terms, representative structure, protein counts, and description."
)]
pub struct InterProLookupInput {
    #[desc = "InterPro accession, for example IPR017861."]
    pub accession: String,
}

pub struct InterProLookupTool {
    pub(crate) client: Arc<InterProClient>,
}

#[async_trait]
impl ToolFunction for InterProLookupTool {
    type Input = InterProLookupInput;

    fn timeout_seconds(&self) -> u64 {
        30
    }

    async fn run(&self, input: Self::Input) -> Result<AgentToolResult, ToolError> {
        let response = self.client.entry(&input.accession).await.map_err(|error| {
            ToolError::ExecutionFailed {
                source: Box::new(error),
            }
        })?;
        Ok(AgentToolResult::success(format_entry(&response)))
    }
}
