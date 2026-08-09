use std::sync::Arc;

use agentik_core::tools::{ToolError, ToolFunction};
use agentik_proc::tool;
use agentik_sdk::types::ToolResult as AgentToolResult;
use async_trait::async_trait;

use crate::OpenAlexClient;
use crate::format::format_autocomplete;

#[tool(
    name = "openalex_autocomplete",
    description = "Fast typeahead / autocomplete search on OpenAlex. \
                  \
                  Use this to resolve entity names into stable OpenAlex IDs \
                  (e.g. search 'Einstein' to get author IDs, or 'Nature' to get source IDs) \
                  before filtering in `openalex_search`. \
                  \
                  `entity` must be one of: 'works', 'authors', 'sources', \
                  'institutions', 'topics', 'funders'."
)]
pub struct OpenAlexAutocompleteInput {
    #[desc = "Entity type to search: 'works', 'authors', 'sources', 'institutions', 'topics', 'funders'."]
    pub entity: String,

    #[desc = "Search query text (what the user has typed so far)."]
    pub query: String,
}

pub struct OpenAlexAutocompleteTool {
    pub(crate) client: Arc<OpenAlexClient>,
}

#[async_trait]
impl ToolFunction for OpenAlexAutocompleteTool {
    type Input = OpenAlexAutocompleteInput;

    fn timeout_seconds(&self) -> u64 {
        30
    }

    async fn run(&self, input: Self::Input) -> Result<AgentToolResult, ToolError> {
        let valid = [
            "works",
            "authors",
            "sources",
            "institutions",
            "topics",
            "funders",
        ];
        if !valid.contains(&input.entity.as_str()) {
            return Err(ToolError::ValidationFailed {
                message: format!(
                    "entity must be one of: {} (got '{}')",
                    valid.join(", "),
                    input.entity
                ),
            });
        }

        let resp = self
            .client
            .autocomplete(&input.entity, &input.query)
            .await
            .map_err(super::json_err)?;

        Ok(AgentToolResult::success(format_autocomplete(&resp)))
    }
}
