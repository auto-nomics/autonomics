use std::sync::Arc;

use agentik_core::tools::{ToolError, ToolFunction};
use agentik_proc::tool;
use agentik_sdk::types::ToolResult as AgentToolResult;
use async_trait::async_trait;

use crate::ProtocolioClient;
use crate::format::format_steps;
use crate::tools::{protocol::parse_format, search::error};

#[tool(
    name = "protocolio_get_steps",
    description = "Fetch ordered protocols.io steps as Markdown. Input may be an ID, URI, DOI, or versioned DOI."
)]
pub struct ProtocolioStepsInput {
    #[desc = "Numeric protocol ID, URI, DOI, DOI/vN, or DOI/latest."]
    pub protocol_id: String,
    #[desc = "Set true to request the latest version explicitly."]
    pub last_version: Option<bool>,
    #[desc = "json, html, or markdown. Defaults to markdown."]
    pub content_format: Option<String>,
}

pub struct ProtocolioStepsTool {
    pub(crate) client: Arc<ProtocolioClient>,
}

#[async_trait]
impl ToolFunction for ProtocolioStepsTool {
    type Input = ProtocolioStepsInput;

    fn timeout_seconds(&self) -> u64 {
        30
    }

    async fn run(&self, input: Self::Input) -> Result<AgentToolResult, ToolError> {
        let format = parse_format(input.content_format.as_deref())?;
        let steps = self
            .client
            .steps(
                &input.protocol_id,
                input.last_version.unwrap_or(false),
                format,
            )
            .await
            .map_err(error)?;
        Ok(AgentToolResult::success(format_steps(
            &input.protocol_id,
            &steps,
        )))
    }
}
