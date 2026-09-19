use std::sync::Arc;

use agentik_core::tools::{ToolError, ToolFunction};
use agentik_proc::tool;
use agentik_sdk::types::ToolResult as AgentToolResult;
use async_trait::async_trait;

use crate::ProtocolioClient;
use crate::format::format_reagents;
use crate::query::ReagentListQuery;
use crate::tools::search::error;

#[tool(
    name = "protocolio_search_reagents",
    description = "Search the protocols.io reagent database by keyword, CiteAB status, and date range."
)]
pub struct ProtocolioReagentsInput {
    #[desc = "Reagent search text."]
    pub query: String,
    #[desc = "Set true for CiteAB reagents only, false to exclude them, or omit for all."]
    pub is_citeab: Option<bool>,
    #[desc = "Unix timestamp lower bound."]
    pub from: Option<i64>,
    #[desc = "Unix timestamp upper bound."]
    pub to: Option<i64>,
    #[desc = "Results per page (1-100). Defaults to 20."]
    pub page_size: Option<u32>,
    #[desc = "One-based page number."]
    pub page_id: Option<u32>,
}

pub struct ProtocolioReagentsTool {
    pub(crate) client: Arc<ProtocolioClient>,
}

#[async_trait]
impl ToolFunction for ProtocolioReagentsTool {
    type Input = ProtocolioReagentsInput;

    fn timeout_seconds(&self) -> u64 {
        30
    }

    async fn run(&self, input: Self::Input) -> Result<AgentToolResult, ToolError> {
        let query = ReagentListQuery {
            key: input.query,
            is_citeab: input.is_citeab,
            from: input.from,
            to: input.to,
            page_size: Some(input.page_size.unwrap_or(20)),
            page_id: input.page_id,
        };
        let response = self.client.list_reagents(&query).await.map_err(error)?;
        Ok(AgentToolResult::success(format_reagents(
            &response.items,
            Some(&response.pagination),
        )))
    }
}
