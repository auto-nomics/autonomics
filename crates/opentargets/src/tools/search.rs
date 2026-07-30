use std::sync::Arc;

use agentik_core::tools::{ToolError, ToolFunction};
use agentik_proc::tool;
use agentik_sdk::types::ToolResult as AgentToolResult;
use async_trait::async_trait;

use super::json_err;
use crate::format::format_search;
use crate::OpenTargetsClient;
use crate::Pagination;

#[tool(
    name = "opentargets_search",
    description = "Full-text search across the Open Targets Platform (genes, diseases, \
                  drugs, studies, variants). Returns ranked hits with entity type, ID, \
                  name, and a relevance score. Use this to resolve an entity name into \
                  its stable ID (Ensembl / EFO / ChEMBL / GCST / chr_pos_ref_alt) \
                  before calling the dedicated lookup tools."
)]
pub struct SearchInput {
    #[desc = "Free-text query, e.g. 'BRCA1', 'Alzheimer', 'atorvastatin'."]
    pub query: String,
    #[desc = "Optional entity filters: 'target', 'disease', 'drug', 'study', 'variant'. \
             If omitted, searches all entity types."]
    pub entity: Option<Vec<String>>,
    #[desc = "Number of results to return (default 20, max 3000)."]
    pub size: Option<u32>,
    #[desc = "0-based page index for pagination (default 0)."]
    pub index: Option<u32>,
}

pub struct SearchTool {
    pub(crate) client: Arc<OpenTargetsClient>,
}

#[async_trait]
impl ToolFunction for SearchTool {
    type Input = SearchInput;

    async fn run(&self, input: Self::Input) -> Result<AgentToolResult, ToolError> {
        let size = input.size.unwrap_or(20).min(3000);
        let index = input.index.unwrap_or(0);
        let entities: Option<Vec<&str>> = input
            .entity
            .as_ref()
            .map(|v| v.iter().map(String::as_str).collect());

        let results = self
            .client
            .search(&input.query, entities.as_deref(), Some(Pagination::new(index, size)))
            .await
            .map_err(json_err)?;

        Ok(AgentToolResult::success(format_search(&results)))
    }
}
