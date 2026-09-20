use std::sync::Arc;

use agentik_core::tools::{ToolError, ToolFunction};
use agentik_proc::tool;
use agentik_sdk::types::ToolResult as AgentToolResult;
use async_trait::async_trait;

use crate::ProtocolioClient;
use crate::format::format_reagents;
use crate::tools::search::error;

#[tool(
    name = "protocolio_get_materials",
    description = "Fetch the reagent and equipment material list for one protocols.io protocol."
)]
pub struct ProtocolioMaterialsInput {
    #[desc = "Numeric protocol ID, URI, DOI, or versioned DOI."]
    pub protocol_id: String,
}

pub struct ProtocolioMaterialsTool {
    pub(crate) client: Arc<ProtocolioClient>,
}

#[async_trait]
impl ToolFunction for ProtocolioMaterialsTool {
    type Input = ProtocolioMaterialsInput;

    fn timeout_seconds(&self) -> u64 {
        30
    }

    async fn run(&self, input: Self::Input) -> Result<AgentToolResult, ToolError> {
        let materials = self
            .client
            .materials(&input.protocol_id)
            .await
            .map_err(error)?;
        Ok(AgentToolResult::success(format_reagents(&materials, None)))
    }
}
