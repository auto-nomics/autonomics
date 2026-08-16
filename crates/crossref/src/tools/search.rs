use std::sync::Arc;

use agentik_core::tools::{ToolError, ToolFunction, ToolResult};
use agentik_proc::tool;
use agentik_sdk::types::ToolResult as AgentToolResult;
use async_trait::async_trait;
use bib_types::StructuredSearch;

use crate::CrossrefClient;
use crate::format::format_works;

/// Search Crossref's 150M+ DOIs for scholarly works.
///
/// TWO query modes (provide exactly one):
///
/// 1. RECOMMENDED — `structured`: a typed query object with named fields
///    (keywords, title, authors, journal, affiliation, publication_types,
///    year_range). The tool translates it to Crossref query syntax for you.
///
///    Example: {
///      "keywords": ["p53", "cancer"],
///      "year_range": {"from": 2020, "to": 2024}
///    }
///
/// 2. EXPERT — `query`: a raw free-text search string for the Crossref
///    `/works` endpoint.
///
///    Example: 'CRISPR gene editing'
#[tool(
    name = "crossref_search",
    description = "Search Crossref (150M+ DOIs: journal articles, books, preprints, conference \
                  papers, datasets, etc.) for scholarly works matching a query. Returns \
                  metadata: title, authors, DOI, journal, year, type, citation count, and \
                  abstract preview. \
                  \
                  TWO query modes (provide exactly one): \
                  \
                 1. RECOMMENDED — `structured`: a typed query object with named fields \
                    (keywords, title, authors, journal, affiliation, publication_types, \
                    year_range). Crossref queries are loose relevance recall: fields are \
                    AND-ed, but strict keywords_op=AND/NOT is unsupported. \
                     \
                     Example: { \
                       \"keywords\": [\"p53\", \"cancer\"], \
                       \"year_range\": {\"from\": 2020, \"to\": 2024} \
                     } \
                  \
                  2. EXPERT — `query`: a raw free-text search for the Crossref /works endpoint. \
                     \
                     Example: 'CRISPR gene editing'"
)]
pub struct CrossrefSearchInput {
    #[desc = "Structured query object. Preferred over `query`. Populate any subset of \
             fields: keywords (all fields), title, authors, journal, affiliation, \
             publication_types, year_range. Fields are AND-ed and keyword terms use loose \
             OR/recall matching; strict AND/NOT is unsupported."]
    pub structured: Option<StructuredSearch>,

    #[desc = "Raw free-text search for the Crossref /works endpoint (expert mode). \
             Used only when `structured` is absent."]
    pub query: Option<String>,

    #[desc = "Maximum number of results to return (default 25, max 1000)."]
    pub rows: Option<u32>,

    #[desc = "Sort field: 'relevance' (default), 'published', 'deposited', 'indexed', \
             'is-referenced-by-count' (most cited), 'references-count'. \
             Use with `order`."]
    pub sort: Option<String>,

    #[desc = "Sort order: 'asc' or 'desc' (default)."]
    pub order: Option<String>,

    #[desc = "Deep-paging cursor for retrieving results beyond 10k. Use '*' for the \
             first page, then pass back the 'next cursor' from the previous response."]
    pub cursor: Option<String>,
}

pub struct CrossrefSearchTool {
    pub client: Arc<CrossrefClient>,
}

#[async_trait]
impl ToolFunction for CrossrefSearchTool {
    type Input = CrossrefSearchInput;

    fn timeout_seconds(&self) -> u64 {
        120
    }

    async fn run(&self, input: Self::Input) -> Result<AgentToolResult, ToolError> {
        let mut works_query = if let Some(sq) = input.structured {
            crate::query::to_crossref_works_query(&sq).map_err(super::json_err)?
        } else if let Some(q) = input.query {
            crate::client::WorksQuery::new().with_query(q)
        } else {
            return Err(ToolError::ValidationFailed {
                message: "crossref_search requires `structured` or `query`".into(),
            });
        };

        if let Some(r) = input.rows {
            works_query = works_query.with_rows(r.min(1000));
        }
        if let Some(ref s) = input.sort {
            works_query = works_query.with_sort(s);
        }
        if let Some(ref o) = input.order {
            works_query = works_query.with_order(o);
        }
        if let Some(ref c) = input.cursor {
            works_query = works_query.with_cursor(c);
        }

        let result = self
            .client
            .works(&works_query)
            .await
            .map_err(super::json_err)?;

        Ok(AgentToolResult::success(format_works(&result)))
    }
}
