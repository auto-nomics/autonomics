use std::sync::Arc;

use agentik_core::tools::{ToolError, ToolFunction, ToolResult};
use agentik_proc::tool;
use agentik_sdk::types::ToolResult as AgentToolResult;
use async_trait::async_trait;

use crate::format::format_details;
use crate::{BiorxivClient, types::Server};

use bib_types::StructuredSearch;

#[tool(
    name = "biorxiv_search",
    description = "Search bioRxiv/medRxiv preprints using a structured query. \
                  \
                  IMPORTANT: The bioRxiv API has NO server-side keyword search. This tool \
                  works by: (1) translating `year_range` into a date interval for the API, \
                  (2) fetching papers in that interval, and (3) applying keyword/title/author \
                  filters CLIENT-SIDE on the fetched results. \
                  \
                  This means large date ranges with specific keywords may require fetching \
                  many pages. For best results, narrow the `year_range` as much as possible. \
                  \
                  Set `server` to 'medrxiv' (health sciences, default) or 'biorxiv' (biology). \
                  \
                  If `year_range` is absent, the last 365 days are searched. \
                  \
                  Populate `keywords` (searched against title + abstract), `title` (title only), \
                  or `authors` (author names). Fields are AND-ed; terms within a field default to \
                  OR (use `keywords_op: \"AND\"` to require all keywords)."
)]
pub struct BiorxivSearchInput {
    #[desc = "Structured query object. Populate any subset of: keywords (title+abstract), \
             title, authors, keywords_op, year_range. Keywords are filtered client-side."]
    pub structured: StructuredSearch,

    #[desc = "Server: 'medrxiv' (default) or 'biorxiv'."]
    pub server: Option<String>,

    #[desc = "Maximum total results to fetch and filter (default 300). Higher values \
             mean more API calls. The tool fetches pages of 100 and filters client-side."]
    pub max_results: Option<usize>,
}

pub struct BiorxivSearchTool {
    pub(crate) client: Arc<BiorxivClient>,
}

#[async_trait]
impl ToolFunction for BiorxivSearchTool {
    type Input = BiorxivSearchInput;

    fn timeout_seconds(&self) -> u64 {
        300
    }

    async fn run(&self, input: Self::Input) -> Result<AgentToolResult, ToolError> {
        let server: Server = input
            .server
            .as_deref()
            .unwrap_or("medrxiv")
            .parse()
            .map_err(|e: String| ToolError::ValidationFailed { message: e })?;

        let max_results = input.max_results.unwrap_or(300);

        // Translate the structured search into an interval + client-side filters.
        let query = crate::query::to_biorxiv(input.structured, server)
            .map_err(super::json_err)?;

        // Fetch all papers in the interval (auto-paginating up to max_results).
        let resp = self
            .client
            .details_all(server, &query.interval, max_results)
            .await
            .map_err(super::json_err)?;

        // Apply client-side filters.
        let filtered: Vec<_> = resp
            .collection
            .iter()
            .filter(|e| query.matches(e))
            .cloned()
            .collect();

        let filtered_resp = crate::types::DetailsResponse {
            messages: vec![crate::types::Message {
                status: "ok".into(),
                total: Some(filtered.len() as u64),
                count: Some(filtered.len() as u64),
                ..Default::default()
            }],
            collection: filtered,
        };

        Ok(AgentToolResult::success(format_details(&filtered_resp)))
    }
}
