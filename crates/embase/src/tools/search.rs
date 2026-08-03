use std::sync::Arc;

use agentik_core::tools::{ToolError, ToolFunction, ToolResult};
use agentik_proc::tool;
use agentik_sdk::types::ToolResult as AgentToolResult;
use async_trait::async_trait;

use crate::{EmbaseClient, format::format_search};

use bib_types::StructuredSearch;

#[tool(
    name = "embase_search",
    description = "Search Embase (Elsevier biomedical literature database) and return matching \
                  records with metadata (title, first author, DOI, journal, abstract preview, \
                  date). Embase covers peer-reviewed biomedical literature, in-press \
                  publications, and conferences with deep indexing using the Emtree thesaurus \
                  (drugs, diseases, medical devices). \
                  \
                  TWO query modes (provide exactly one): \
                  \
                  1. RECOMMENDED — `structured`: a typed query object with named fields \
                     (keywords, title, authors, mesh, journal, publication_types, affiliation, \
                     year_range). The tool translates it to Embase CommandLanguage syntax for \
                     you. \
                     \
                     Example: { \
                       \"keywords\": [\"CRISPR\", \"gene editing\"], \
                       \"keywords_op\": \"OR\", \
                       \"publication_types\": [\"Review\"], \
                       \"year_range\": {\"from\": 2020, \"to\": 2024} \
                     } \
                  \
                  2. EXPERT — `query`: a raw Embase CommandLanguage query expression. \
                     \
                     Field tags (appended after a colon): :ti (title), :ab (abstract), \
                     :ti,ab (title+abstract), :au (author), :ta (journal/source title), \
                     :dt (document type), :de (Emtree descriptor), :af (affiliation), \
                     :py (publication year). \
                     Boolean operators (uppercase): AND, OR, NOT. \
                     Phrase-quote with single quotes: 'heart attack':ti,ab. \
                     \
                     Example: \"'heart attack':ti,ab AND aspirin:ti,ab\""
)]
pub struct EmbaseSearchInput {
    #[desc = "Structured query object. Preferred over `query`. Populate any subset of \
             fields: keywords (Title/Abstract), title, authors, mesh (Emtree terms), \
             journal, publication_types, affiliation, year_range. Fields are AND-ed \
             together; terms within a field are OR-ed (use keywords_op to change \
             keywords' join)."]
    pub structured: Option<StructuredSearch>,

    #[desc = "Raw Embase CommandLanguage query expression (expert mode). Used only when \
             `structured` is absent. Example: \"'CRISPR':ti,ab AND 'gene editing':ti,ab\"."]
    pub query: Option<String>,

    #[desc = "Maximum number of results to return (default 25)."]
    pub count: Option<u32>,

    #[desc = "Start position for pagination (1-based, default 1)."]
    pub start: Option<u32>,

    #[desc = "Sort order: 'relevance' (default), 'entrydate' (newest entries first), or \
             'publicationyear'."]
    pub sort: Option<String>,
}

pub struct EmbaseSearchTool {
    pub(crate) client: Arc<EmbaseClient>,
}

#[async_trait]
impl ToolFunction for EmbaseSearchTool {
    type Input = EmbaseSearchInput;

    fn timeout_seconds(&self) -> u64 {
        120
    }

    async fn run(&self, input: Self::Input) -> Result<AgentToolResult, ToolError> {
        // Resolve the query term: structured mode takes precedence, then raw
        // `query`. Reject calls that provide neither.
        let query = if let Some(sq) = input.structured {
            crate::query::to_embase(&sq).map_err(super::json_err)?
        } else if let Some(q) = input.query {
            q
        } else {
            return Err(ToolError::ValidationFailed {
                message: "embase_search requires `structured` or `query`".into(),
            });
        };

        let req = crate::types::SearchRequest {
            query,
            count: input.count,
            start: input.start,
            sort: input.sort,
            alert_id: None,
        };

        let result = self.client.search(&req).await.map_err(super::json_err)?;

        Ok(AgentToolResult::success(format_search(&result)))
    }
}
