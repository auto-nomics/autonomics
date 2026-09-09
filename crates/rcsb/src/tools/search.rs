use std::sync::Arc;

use agentik_core::tools::{ToolError, ToolFunction};
use agentik_proc::tool;
use agentik_sdk::types::ToolResult;
use async_trait::async_trait;

use crate::RcsbClient;
use crate::format::format_search_summaries;
use crate::search::{SearchQuery, SearchRequest, SearchReturnType};

#[tool(
    name = "rcsb_search",
    description = "Search RCSB PDB structures and return concise entry summaries. Provide query for full-text search, or raw_query for an expert RCSB Search API query-node object. Top entry results are enriched with titles, methods, resolutions, and entity counts by default."
)]
pub struct RcsbSearchInput {
    #[desc = "Full-text query, e.g. 'human hemoglobin crystal structure'."]
    pub query: Option<String>,

    #[desc = "Expert-mode RCSB query node containing type, service, and parameters."]
    pub raw_query: Option<serde_json::Value>,

    #[desc = "Maximum results (default 10, capped at 25)."]
    pub rows: Option<u32>,

    #[desc = "Zero-based offset for the next page."]
    pub start: Option<u32>,

    #[desc = "entry, polymer_entity, polymer_entity_instance, assembly, or chem_comp."]
    pub return_type: Option<String>,

    #[desc = "Fetch entry metadata for top hits. Only valid for entry return_type."]
    pub include_details: Option<bool>,
}

pub struct RcsbSearchTool {
    pub client: Arc<RcsbClient>,
}

#[async_trait]
impl ToolFunction for RcsbSearchTool {
    type Input = RcsbSearchInput;

    fn timeout_seconds(&self) -> u64 {
        120
    }

    async fn run(&self, input: Self::Input) -> Result<ToolResult, ToolError> {
        let query = if let Some(raw) = input.raw_query {
            serde_json::from_value::<SearchQuery>(raw).map_err(|e| ToolError::ValidationFailed {
                message: format!("invalid raw_query: {e}"),
            })?
        } else if let Some(query) = input.query {
            if query.trim().is_empty() {
                return Err(ToolError::ValidationFailed {
                    message: "rcsb_search requires a non-empty query".into(),
                });
            }
            SearchQuery::full_text(query.trim())
        } else {
            return Err(ToolError::ValidationFailed {
                message: "rcsb_search requires `query` or `raw_query`".into(),
            });
        };

        let return_type = parse_return_type(input.return_type.as_deref())?;
        let mut request = SearchRequest {
            query,
            return_type,
            request_options: crate::search::SearchRequestOptions {
                paginate: Default::default(),
                results_content_type: vec!["experimental".into()],
            },
        };
        request.request_options.paginate.start = input.start.unwrap_or(0);
        request.request_options.paginate.rows = input.rows.unwrap_or(10).min(25);

        let response = self
            .client
            .search(&request)
            .await
            .map_err(super::tool_error)?;
        let include_details = input.include_details.unwrap_or(true);
        let entries = if include_details && matches!(return_type, SearchReturnType::Entry) {
            let ids = response
                .result_set
                .iter()
                .map(|result| result.identifier.clone());
            self.client.entries(ids).await.map_err(super::tool_error)?
        } else {
            Vec::new()
        };

        let mut markdown = format_search_summaries(&response, &entries);
        let shown = response.result_set.len() as u32;
        if shown > 0
            && response.total_count > (request.request_options.paginate.start + shown) as u64
        {
            markdown.push_str(&format!(
                "\n\n_Next page:_ `start={}`\n",
                request.request_options.paginate.start + shown
            ));
        }
        Ok(ToolResult::success(markdown))
    }
}

fn parse_return_type(value: Option<&str>) -> Result<SearchReturnType, ToolError> {
    let Some(value) = value else {
        return Ok(SearchReturnType::Entry);
    };
    match value.trim().to_ascii_lowercase().as_str() {
        "entry" => Ok(SearchReturnType::Entry),
        "polymer_entity" | "polymer" => Ok(SearchReturnType::PolymerEntity),
        "polymer_entity_instance" | "instance" => Ok(SearchReturnType::PolymerEntityInstance),
        "assembly" => Ok(SearchReturnType::Assembly),
        "chem_comp" | "chemical_component" => Ok(SearchReturnType::ChemComp),
        other => Err(ToolError::ValidationFailed {
            message: format!("unsupported return_type {other:?}"),
        }),
    }
}
