use std::sync::Arc;

use agentik_core::tools::{ToolError, ToolFunction, ToolResult};
use agentik_proc::tool;
use agentik_sdk::types::ToolResult as AgentToolResult;
use async_trait::async_trait;

use crate::UniProtClient;
use crate::format::format_proteomes;
use crate::types::SearchRequest;

#[tool(
    name = "uniprot_proteomes",
    description = "Search UniProt reference proteomes (organism-level protein sets). \
                  \
                  Provide `organism_id` (e.g. 9606) and/or a free-text `query` over \
                  organism names. Results list the proteome UPID, organism, proteome \
                  type, and protein counts. Use the UPID with the stream endpoints \
                  to download a whole proteome (e.g. query 'proteome:UP000005640')."
)]
pub struct UniprotProteomesInput {
    #[desc = "NCBI taxon ID of the organism, e.g. 9606 (human), 10090 (mouse)."]
    pub organism_id: Option<u64>,

    #[desc = "Raw proteomes query expression (expert mode), e.g. \
             'proteome_type:reference'. Used only when `organism_id` is absent."]
    pub query: Option<String>,

    #[desc = "Maximum number of proteomes to return (default 25, max 500)."]
    pub size: Option<u32>,

    #[desc = "Cursor from a previous response to fetch the next page."]
    pub cursor: Option<String>,
}

pub struct UniprotProteomesTool {
    pub(crate) client: Arc<UniProtClient>,
}

#[async_trait]
impl ToolFunction for UniprotProteomesTool {
    type Input = UniprotProteomesInput;

    fn timeout_seconds(&self) -> u64 {
        120
    }

    async fn run(&self, input: Self::Input) -> Result<AgentToolResult, ToolError> {
        let mut req = match (input.organism_id, input.query) {
            (Some(id), _) => SearchRequest::new(format!("organism_id:{id}")),
            (None, Some(q)) if !q.trim().is_empty() => SearchRequest::new(q),
            (None, _) => {
                return Err(ToolError::ValidationFailed {
                    message: "uniprot_proteomes requires `organism_id` or `query`".into(),
                });
            }
        };

        if let Some(size) = input.size {
            req = req.size(size);
        }
        if let Some(cursor) = input.cursor {
            req = req.cursor(cursor);
        }

        let page = self
            .client
            .search_proteomes(&req)
            .await
            .map_err(super::json_err)?;
        Ok(AgentToolResult::success(format_proteomes(&page)))
    }
}
