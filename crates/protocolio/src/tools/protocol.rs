use std::sync::Arc;

use agentik_core::tools::{ToolError, ToolFunction};
use agentik_proc::tool;
use agentik_sdk::types::ToolResult as AgentToolResult;
use async_trait::async_trait;

use crate::ProtocolioClient;
use crate::format::format_protocol;
use crate::tools::search::error;
use crate::types::ContentFormat;

#[tool(
    name = "protocolio_get_protocol",
    description = "Fetch one protocols.io record by numeric ID, URI, DOI, DOI/version, or latest."
)]
pub struct ProtocolioGetInput {
    #[desc = "Numeric protocol ID, URI, DOI, DOI/vN, or DOI/latest."]
    pub protocol_id: String,
    #[desc = "Set true to request the latest version explicitly."]
    pub last_version: Option<bool>,
    #[desc = "json, html, or markdown. Defaults to markdown."]
    pub content_format: Option<String>,
}

pub struct ProtocolioGetTool {
    pub(crate) client: Arc<ProtocolioClient>,
}

#[async_trait]
impl ToolFunction for ProtocolioGetTool {
    type Input = ProtocolioGetInput;

    fn timeout_seconds(&self) -> u64 {
        30
    }

    async fn run(&self, input: Self::Input) -> Result<AgentToolResult, ToolError> {
        let format = parse_format(input.content_format.as_deref())?;
        let protocol = self
            .client
            .protocol(
                &input.protocol_id,
                input.last_version.unwrap_or(false),
                format,
            )
            .await
            .map_err(error)?;
        Ok(AgentToolResult::success(format_protocol(&protocol)))
    }
}

pub(crate) fn parse_format(value: Option<&str>) -> Result<ContentFormat, ToolError> {
    value
        .map(ContentFormat::parse)
        .unwrap_or_default()
        .ok_or_else(|| ToolError::ValidationFailed {
            message: "content_format must be json, html, or markdown".to_owned(),
        })
}
