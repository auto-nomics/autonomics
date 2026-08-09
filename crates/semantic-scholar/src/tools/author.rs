use std::sync::Arc;

use agentik_core::tools::{ToolError, ToolFunction};
use agentik_proc::tool;
use agentik_sdk::types::ToolResult as AgentToolResult;
use async_trait::async_trait;

use crate::{
    S2Client,
    format::{format_author_detail, format_author_search},
};

#[tool(
    name = "s2_author",
    description = "Search Semantic Scholar for authors by name, or get details about \
                  a specific author by their S2 author ID. \
                  \
                  Provide `query` to search (returns matching authors with paper \
                  counts, citation counts, h-index, affiliations). \
                  Provide `author_id` to get a single author's details. \
                  \
                  Examples: \
                  - Search: { \"query\": \"Andrew Ng\" } \
                  - Details: { \"author_id\": \"1741101\" }"
)]
pub struct S2AuthorInput {
    #[desc = "Author name to search for. Returns ranked matches with metadata. \
             Used when `author_id` is absent."]
    pub query: Option<String>,

    #[desc = "Semantic Scholar author ID. Returns a single author's full details. \
             Takes priority over `query`."]
    pub author_id: Option<String>,

    #[desc = "Maximum number of results for author search (default 10, max 1000)."]
    pub limit: Option<u32>,
}

pub struct S2AuthorTool {
    pub(crate) client: Arc<S2Client>,
}

#[async_trait]
impl ToolFunction for S2AuthorTool {
    type Input = S2AuthorInput;

    fn timeout_seconds(&self) -> u64 {
        60
    }

    async fn run(&self, input: Self::Input) -> Result<AgentToolResult, ToolError> {
        // Single author detail takes priority.
        if let Some(id) = input.author_id {
            let author = self
                .client
                .get_author(&id, None)
                .await
                .map_err(super::json_err)?;
            return Ok(AgentToolResult::success(format_author_detail(&author)));
        }

        // Otherwise search.
        let query = input.query.ok_or_else(|| ToolError::ValidationFailed {
            message: "s2_author requires `query` or `author_id`".into(),
        })?;

        let resp = self
            .client
            .search_authors(&query, input.limit.unwrap_or(10), 0, None)
            .await
            .map_err(super::json_err)?;
        Ok(AgentToolResult::success(format_author_search(&resp)))
    }
}
