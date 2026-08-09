use std::sync::Arc;

use agentik_core::tools::{ToolError, ToolFunction};
use agentik_proc::tool;
use agentik_sdk::types::ToolResult as AgentToolResult;
use async_trait::async_trait;
use bib_types::StructuredSearch;

use crate::format::format_works;
use crate::{ListParams, OpenAlexClient};

#[tool(
    name = "openalex_search",
    description = "Search OpenAlex (270M+ scholarly works: articles, books, datasets, preprints) \
                  and return matching records with metadata. \
                  \
                  TWO query modes (provide exactly one): \
                  \
                  1. RECOMMENDED — `structured`: a typed query object with named fields \
                     (keywords, title, authors, mesh, journal, publication_types, \
                     affiliation, year_range). The tool translates it to OpenAlex \
                     filter syntax for you. \
                     \
                     Example: { \
                       \"keywords\": [\"CRISPR\", \"gene editing\"], \
                       \"keywords_op\": \"AND\", \
                       \"year_range\": {\"from\": 2020, \"to\": 2024} \
                     } \
                  \
                  2. EXPERT — `filter`: a raw OpenAlex filter expression (field:value pairs \
                     joined by commas). \
                     \
                     Common filters: publication_year:2024, type:article, is_oa:true, \
                     cited_by_count:>100, authorships.author.id:A5023888391, \
                     primary_location.source.id:S123, open_access.is_oa:true. \
                     \
                     OR within a field uses pipe: type:article|book-chapter. \
                     NOT: type:!paratext. Range: publication_year:2020-2024. \
                     \
                     Example: 'publication_year:2024,is_oa:true,cited_by_count:>100'"
)]
pub struct OpenAlexSearchInput {
    #[desc = "Structured query object. Preferred over `filter`. Populate any subset of \
             fields: keywords (full-text search), title, authors, mesh, journal, \
             publication_types, affiliation, year_range. Fields are AND-ed together."]
    pub structured: Option<StructuredSearch>,

    #[desc = "Raw OpenAlex filter expression (expert mode). Used only when \
             `structured` is absent. Example: 'publication_year:2024,type:article,cited_by_count:>100'."]
    pub filter: Option<String>,

    #[desc = "Sort expression, e.g. 'cited_by_count:desc' (most cited first), \
             'publication_date:desc' (newest first). Default: relevance."]
    pub sort: Option<String>,

    #[desc = "Maximum number of results to return (default 25, max 100)."]
    pub per_page: Option<u32>,

    #[desc = "Cursor for deep pagination. Use '*' for first page, then pass back \
             the next_cursor from the previous response. Default: not set (basic paging)."]
    pub cursor: Option<String>,

    #[desc = "Comma-separated list of fields to include (reduces payload). \
             Example: 'id,doi,title,publication_year,cited_by_count,authorships'. \
             Default: all fields."]
    pub select: Option<String>,
}

pub struct OpenAlexSearchTool {
    pub(crate) client: Arc<OpenAlexClient>,
}

#[async_trait]
impl ToolFunction for OpenAlexSearchTool {
    type Input = OpenAlexSearchInput;

    fn timeout_seconds(&self) -> u64 {
        120
    }

    async fn run(&self, input: Self::Input) -> Result<AgentToolResult, ToolError> {
        let mut params = ListParams::new();

        if let Some(per_page) = input.per_page {
            params = params.with_per_page(per_page.min(100));
        }
        if let Some(ref sort) = input.sort {
            params = params.with_sort(sort);
        }
        if let Some(ref cursor) = input.cursor {
            params = params.with_cursor(cursor);
        }
        if let Some(ref select) = input.select {
            params = params.with_select(select);
        }

        if let Some(sq) = input.structured {
            let filter = crate::query::to_openalex_filter(&sq).map_err(super::json_err)?;
            if !filter.is_empty() {
                params = params.with_filter(filter);
            }
            // Keywords become the `search` param.
            if let Some(kw_search) = crate::query::keywords_search(&sq) {
                params = params.with_search(kw_search);
            }
        } else if let Some(filter) = input.filter {
            params = params.with_filter(filter);
        } else {
            return Err(ToolError::ValidationFailed {
                message: "openalex_search requires `structured` or `filter`".into(),
            });
        }

        let result = self
            .client
            .list_works(&params)
            .await
            .map_err(super::json_err)?;

        Ok(AgentToolResult::success(format_works(&result)))
    }
}
