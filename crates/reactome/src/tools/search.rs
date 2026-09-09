use std::sync::Arc;

use agentik_core::tools::{ToolError, ToolFunction};
use agentik_proc::tool;
use agentik_sdk::types::ToolResult as AgentToolResult;
use async_trait::async_trait;

use super::json_err;
use crate::ReactomeClient;
use crate::format::format_search;

#[tool(
    name = "reactome_search",
    description = "Full-text search across Reactome pathways, reactions, proteins, \
                  chemicals, and diseases."
)]
pub struct SearchInput {
    #[desc = "Search term or phrase, e.g. 'TP53' or 'apoptosis'."]
    pub query: String,

    #[desc = "Optional species filter, e.g. 'Homo sapiens'."]
    pub species: Option<String>,

    #[desc = "Optional type filter(s), e.g. ['Pathway', 'Protein']. Types: \
              Pathway, Reaction, Protein, Chemical, Disease, Set, Complex, GO_BiologicalProcess."]
    pub types: Option<Vec<String>>,
}

pub struct ReactomeSearchTool {
    pub(crate) client: Arc<ReactomeClient>,
}

#[async_trait]
impl ToolFunction for ReactomeSearchTool {
    type Input = SearchInput;

    async fn run(&self, input: Self::Input) -> Result<AgentToolResult, ToolError> {
        let result = self
            .client
            .search(
                &input.query,
                input.species.as_deref(),
                input
                    .types
                    .as_deref()
                    .map(|v| v.iter().map(String::as_str).collect::<Vec<&str>>())
                    .as_deref(),
            )
            .await
            .map_err(json_err)?;
        Ok(AgentToolResult::success(format_search(
            &input.query,
            &result,
        )))
    }
}
