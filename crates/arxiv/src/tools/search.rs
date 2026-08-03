use std::sync::Arc;

use agentik_core::tools::{ToolError, ToolFunction, ToolResult};
use agentik_proc::tool;
use agentik_sdk::types::ToolResult as AgentToolResult;
use async_trait::async_trait;

use crate::{ArxivClient, format::format_search};

use bib_types::StructuredSearch;

#[tool(
    name = "arxiv_search",
    description = "Search arXiv preprint server and return matching papers with \
                  metadata (title, authors, abstract preview, categories, DOI). \
                  \
                  TWO query modes (provide exactly one): \
                  \
                  1. RECOMMENDED — `structured`: a typed query object with named fields \
                     (keywords, title, authors, journal, year_range, …). The tool \
                     translates it to arXiv query syntax for you. \
                     \
                     Example: { \
                       \"keywords\": [\"transformer\", \"attention\"], \
                       \"keywords_op\": \"AND\", \
                       \"year_range\": {\"from\": 2020, \"to\": 2024} \
                     } \
                  \
                  2. EXPERT — `term`: a raw arXiv query expression using field prefixes \
                     and boolean operators. \
                     \
                     Field prefixes: all: (all fields), ti: (title), au: (author), \
                     abs: (abstract), cat: (category), jr: (journal ref), co: (comment). \
                     Boolean operators (uppercase): AND, OR, ANDNOT. \
                     Group with parentheses. Phrase-quote with double quotes. \
                     \
                     Example: 'cat:cs.LG AND ti:transformer ANDNOT abs:survey'"
)]
pub struct ArxivSearchInput {
    #[desc = "Structured query object. Preferred over `term`. Populate any subset of \
             fields: keywords (all fields), title, authors, journal, publication_types, \
             mesh, affiliation, year_range. Fields are AND-ed together; terms within a \
             field are OR-ed (use keywords_op to change keywords' join)."]
    pub structured: Option<StructuredSearch>,

    #[desc = "Raw arXiv query expression (expert mode). Used only when `structured` \
             is absent. Example: 'au:Hinton AND ti:neural network'."]
    pub term: Option<String>,

    #[desc = "Maximum number of results to return (default 10, max 2000)."]
    pub max_results: Option<u32>,

    #[desc = "Start index for pagination (0-based)."]
    pub start: Option<u32>,

    #[desc = "Sort by: 'relevance' (default), 'lastUpdatedDate', 'submittedDate'."]
    pub sort_by: Option<String>,

    #[desc = "Sort order: 'descending' (default, newest first) or 'ascending'."]
    pub sort_order: Option<String>,
}

pub struct ArxivSearchTool {
    pub(crate) client: Arc<ArxivClient>,
}

#[async_trait]
impl ToolFunction for ArxivSearchTool {
    type Input = ArxivSearchInput;

    fn timeout_seconds(&self) -> u64 {
        120
    }

    async fn run(&self, input: Self::Input) -> Result<AgentToolResult, ToolError> {
        // Resolve the query term: structured mode takes precedence, then raw
        // `term`. Reject calls that provide neither.
        let search_query = if let Some(sq) = input.structured {
            crate::query::to_arxiv(&sq).map_err(super::json_err)?
        } else if let Some(t) = input.term {
            t
        } else {
            return Err(ToolError::ValidationFailed {
                message: "arxiv_search requires `structured` or `term`".into(),
            });
        };

        let sort_by = input.sort_by.as_deref().map(|s| match s {
            "lastUpdatedDate" | "last_updated" => crate::types::SortBy::LastUpdatedDate,
            "submittedDate" | "submitted" => crate::types::SortBy::SubmittedDate,
            _ => crate::types::SortBy::Relevance,
        });

        let sort_order = input.sort_order.as_deref().map(|s| match s {
            "ascending" | "asc" => crate::types::SortOrder::Ascending,
            _ => crate::types::SortOrder::Descending,
        });

        let req = crate::types::SearchRequest {
            search_query,
            max_results: input.max_results,
            start: input.start,
            sort_by,
            sort_order,
        };

        let result = self.client.search(&req).await.map_err(super::json_err)?;

        Ok(AgentToolResult::success(format_search(&result)))
    }
}
