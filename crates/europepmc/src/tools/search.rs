use std::sync::Arc;

use agentik_core::tools::{ToolError, ToolFunction, ToolResult};
use agentik_proc::tool;
use agentik_sdk::types::ToolResult as AgentToolResult;
use async_trait::async_trait;

use crate::{EuropePmcClient, format::format_search};

use bib_types::StructuredSearch;

#[tool(
    name = "europepmc_search",
    description = "Search Europe PMC (48M+ life-sciences publications: PubMed, patents, \
                  preprints, books) and return matching records with metadata. \
                  \
                  TWO query modes (provide exactly one): \
                  \
                  1. RECOMMENDED — `structured`: a typed query object with named fields \
                     (keywords, title, authors, mesh, journal, publication_types, \
                     affiliation, year_range). The tool translates it to Europe PMC query \
                     syntax for you. \
                     \
                     Example: { \
                       \"keywords\": [\"p53\", \"cancer\"], \
                       \"keywords_op\": \"AND\", \
                       \"year_range\": {\"from\": 2020, \"to\": 2024} \
                     } \
                  \
                  2. EXPERT — `query`: a raw Europe PMC query expression. \
                     \
                     Field prefixes: TITLE: (title), AUTH: (author), ABSTRACT: (abstract), \
                     KEYWORD: (keyword), JOURNAL: (journal), MESH: (MeSH), AFF: (affiliation), \
                     PUB_TYPE: (publication type), PUB_YEAR: (publication year). \
                     Bare terms search all fields. \
                     Boolean operators (uppercase): AND, OR, NOT. \
                     Phrase-quote with double quotes: \"lung cancer\". \
                     \
                     Example: 'AUTH:Smith AND TITLE:p53 AND PUB_TYPE:Review'"
)]
pub struct EuropePmcSearchInput {
    #[desc = "Structured query object. Preferred over `query`. Populate any subset of \
             fields: keywords (all fields), title, authors, mesh, journal, \
             publication_types, affiliation, year_range. Fields are AND-ed together; \
             terms within a field are OR-ed (use keywords_op to change keywords' join)."]
    pub structured: Option<StructuredSearch>,

    #[desc = "Raw Europe PMC query expression (expert mode). Used only when \
             `structured` is absent. Example: 'AUTH:Smith AND TITLE:p53'."]
    pub query: Option<String>,

    #[desc = "Result detail level: 'lite' (key metadata, default) or 'core' (full \
             metadata with abstract, MeSH terms, full author list)."]
    pub result_type: Option<String>,

    #[desc = "Maximum number of results to return (default 25, max 1000)."]
    pub page_size: Option<u32>,

    #[desc = "Cursor mark for pagination. Use '*' for first page, then pass back \
             the nextCursorMark from the previous response. Default: '*'."]
    pub cursor_mark: Option<String>,

    #[desc = "Sort expression, e.g. 'CITED desc' (most cited first), 'P_PDATE_D desc' \
             (newest first). Default: relevance."]
    pub sort: Option<String>,
}

pub struct EuropePmcSearchTool {
    pub(crate) client: Arc<EuropePmcClient>,
}

#[async_trait]
impl ToolFunction for EuropePmcSearchTool {
    type Input = EuropePmcSearchInput;

    fn timeout_seconds(&self) -> u64 {
        120
    }

    async fn run(&self, input: Self::Input) -> Result<AgentToolResult, ToolError> {
        let query = if let Some(sq) = input.structured {
            crate::query::to_europepmc(&sq).map_err(super::json_err)?
        } else if let Some(q) = input.query {
            q
        } else {
            return Err(ToolError::ValidationFailed {
                message: "europepmc_search requires `structured` or `query`".into(),
            });
        };

        let result_type = match input.result_type.as_deref() {
            Some("core") | Some("CORE") => crate::types::ResultType::Core,
            Some("idlist") | Some("IDLIST") => crate::types::ResultType::Idlist,
            _ => crate::types::ResultType::Lite,
        };

        let req = crate::types::SearchRequest {
            query,
            result_type,
            synonym: false,
            cursor_mark: input.cursor_mark.or_else(|| Some("*".to_owned())),
            page_size: input.page_size,
            sort: input.sort,
        };

        let result = self.client.search(&req).await.map_err(super::json_err)?;

        Ok(AgentToolResult::success(format_search(&result)))
    }
}
