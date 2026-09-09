use std::sync::Arc;

use agentik_core::tools::{ToolError, ToolFunction};
use agentik_proc::tool;
use agentik_sdk::types::ToolResult as AgentToolResult;
use async_trait::async_trait;

use super::json_err;
use crate::ReactomeClient;
use crate::format::format_participants;

#[tool(
    name = "reactome_participants",
    description = "List physical entity participants of a Reactome pathway or reaction, \
                  with their cross-referenced identifiers."
)]
pub struct ParticipantsInput {
    #[desc = "Reactome stable ID (e.g. 'R-HSA-1640170') or numeric dbId."]
    pub id: String,
}

pub struct ReactomeParticipantsTool {
    pub(crate) client: Arc<ReactomeClient>,
}

#[async_trait]
impl ToolFunction for ReactomeParticipantsTool {
    type Input = ParticipantsInput;

    async fn run(&self, input: Self::Input) -> Result<AgentToolResult, ToolError> {
        let participants = self
            .client
            .participants(&input.id)
            .await
            .map_err(json_err)?;
        Ok(AgentToolResult::success(format_participants(
            &input.id,
            &participants,
        )))
    }
}
