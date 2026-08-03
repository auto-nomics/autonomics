use std::sync::Arc;

use agentik_core::tools::{ToolError, ToolFunction, ToolResult};
use agentik_proc::tool;
use agentik_sdk::types::ToolResult as AgentToolResult;
use async_trait::async_trait;

use crate::{ArxivClient, convert::atom_to_articles, format::format_entries_full};

#[tool(
    name = "arxiv_fetch",
    description = "Fetch full arXiv paper metadata by arXiv ID(s). Returns complete \
                  article records including title, authors with affiliations, full \
                  abstract, DOI, journal reference, categories, and PDF link. \
                  More detailed than arxiv_search."
)]
pub struct ArxivFetchInput {
    #[desc = "arXiv ID(s) as a comma-separated string (e.g. '2401.12345,2309.01234v2'). \
             Old-style IDs like 'cond-mat/0207270' are also accepted. Append vN \
             for a specific version."]
    pub id: String,
}

pub struct ArxivFetchTool {
    pub(crate) client: Arc<ArxivClient>,
}

#[async_trait]
impl ToolFunction for ArxivFetchTool {
    type Input = ArxivFetchInput;

    fn timeout_seconds(&self) -> u64 {
        120
    }

    async fn run(&self, input: Self::Input) -> Result<AgentToolResult, ToolError> {
        let req = crate::types::FetchRequest {
            id_list: input.id,
            max_results: None,
            start: None,
        };

        let resp = self
            .client
            .fetch_by_id(&req)
            .await
            .map_err(super::json_err)?;

        // Also convert to typed Articles for programmatic use.
        let _articles = atom_to_articles(&resp.entries);

        Ok(AgentToolResult::success(format_entries_full(
            &resp.entries,
        )))
    }
}
