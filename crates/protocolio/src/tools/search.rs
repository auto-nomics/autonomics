use std::sync::Arc;

use agentik_core::tools::{ToolError, ToolFunction};
use agentik_proc::tool;
use agentik_sdk::types::ToolResult as AgentToolResult;
use async_trait::async_trait;

use crate::ProtocolioClient;
use crate::format::format_summaries;
use crate::query::ProtocolListQuery;

#[tool(
    name = "protocolio_search_protocols",
    description = "Search protocols.io by title, description, or authors. Supports public, \
                  user-public, private, and shared-with-user filters."
)]
pub struct ProtocolioSearchInput {
    #[desc = "Search text. Enclose an exact phrase in double quotes."]
    pub query: String,
    #[desc = "public, user_public, user_private, or shared_with_user. Defaults to public."]
    pub filter: Option<String>,
    #[desc = "activity, relevance, date, name, or id."]
    pub order_field: Option<String>,
    #[desc = "asc or desc."]
    pub order_dir: Option<String>,
    #[desc = "Results per page (1-100). Defaults to 20."]
    pub page_size: Option<u32>,
    #[desc = "One-based page number."]
    pub page_id: Option<u32>,
    #[desc = "Set true to include only peer-reviewed protocols."]
    pub peer_reviewed: Option<bool>,
}

pub struct ProtocolioSearchTool {
    pub(crate) client: Arc<ProtocolioClient>,
}

#[async_trait]
impl ToolFunction for ProtocolioSearchTool {
    type Input = ProtocolioSearchInput;

    fn timeout_seconds(&self) -> u64 {
        30
    }

    async fn run(&self, input: Self::Input) -> Result<AgentToolResult, ToolError> {
        let query = ProtocolListQuery {
            filter: Some(input.filter.unwrap_or_else(|| "public".to_owned())),
            key: input.query,
            order_field: input.order_field,
            order_dir: input.order_dir,
            page_size: Some(input.page_size.unwrap_or(20)),
            page_id: input.page_id,
            peer_reviewed: input.peer_reviewed,
            ..Default::default()
        };
        let response = self.client.list_protocols(&query).await.map_err(error)?;
        Ok(AgentToolResult::success(format_summaries(
            &response.items,
            &response.pagination,
        )))
    }
}

pub(crate) fn error(error: crate::ProtocolioError) -> ToolError {
    ToolError::ExecutionFailed {
        source: Box::new(error),
    }
}
