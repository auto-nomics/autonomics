use std::sync::Arc;

use agentik_core::tools::{ToolError, ToolFunction, ToolResult};
use agentik_proc::tool;
use agentik_sdk::types::ToolResult as AgentToolResult;
use async_trait::async_trait;

use crate::{EutilsClient, format::format_esearch};

use bib_types::StructuredSearch;

#[tool(
    name = "pubmed_search",
    description = "Search PubMed and return matching PMIDs, total count, and history \
                  info for chaining into pubmed_fetch or pubmed_summary. Supports date \
                  filters, sort orders, and pagination. \
                  \
                  TWO query modes (provide exactly one): \
                  \
                  1. RECOMMENDED — `structured`: a typed query object with named fields \
                     (keywords, authors, mesh, journal, publication_types, year_range, …). \
                     The tool translates it to Entrez syntax for you, so you never have to \
                     remember field tags or worry about quoting. \
                     \
                     Example: { \
                       \"keywords\": [\"CRISPR\", \"gene editing\"], \
                       \"keywords_op\": \"OR\", \
                       \"publication_types\": [\"Review\"], \
                       \"year_range\": {\"from\": 2020, \"to\": 2024} \
                     } \
                  \
                  2. EXPERT — `term`: a raw Entrez query expression using field tags \
                     and boolean operators. Use this only when you need features the \
                     structured mode does not cover (nested boolean groups, MeSH tree \
                     codes, etc.). \
                     \
                     Field tags: [Title/Abstract], [Title], [Author], [MeSH], \
                     [Publication Type], [Journal], [Affiliation], [Year]. \
                     Boolean operators (uppercase): AND, OR, NOT."
)]
pub struct PubmedSearchInput {
    #[desc = "Structured query object. Preferred over `term`. Populate any subset of \
             fields: keywords (Title/Abstract), title, authors, mesh, journal, \
             publication_types, affiliation, year_range. Fields are AND-ed together; \
             terms within a field are OR-ed (use keywords_op to change keywords' join)."]
    pub structured: Option<StructuredSearch>,

    #[desc = "Raw Entrez query expression (expert mode). Used only when `structured` \
             is absent. Example: '(cancer OR tumor)[Title/Abstract] AND review[Publication Type]'."]
    pub term: Option<String>,

    #[desc = "Maximum number of results to return (default 20, max 10000)."]
    pub retmax: Option<u32>,
    #[desc = "Start index for pagination (0-based)."]
    pub retstart: Option<u32>,
    #[desc = "Sort order: 'relevance', 'pub_date', 'Author', 'JournalName'."]
    pub sort: Option<String>,
    #[desc = "Date filter type: 'pdat' (publication), 'mdat' (modification), 'edat' (Entrez)."]
    pub datetype: Option<String>,
    #[desc = "Filter to records within the last N days (used with datetype)."]
    pub reldate: Option<u32>,
    #[desc = "Minimum date for range filter, format YYYY/MM/DD (used with datetype)."]
    pub mindate: Option<String>,
    #[desc = "Maximum date for range filter, format YYYY/MM/DD (used with datetype)."]
    pub maxdate: Option<String>,
}

pub struct PubmedSearchTool {
    pub(crate) client: Arc<EutilsClient>,
}

#[async_trait]
impl ToolFunction for PubmedSearchTool {
    type Input = PubmedSearchInput;

    fn timeout_seconds(&self) -> u64 {
        120
    }

    async fn run(&self, input: Self::Input) -> Result<AgentToolResult, ToolError> {
        // Resolve the query term: structured mode takes precedence, then raw
        // `term`. Reject calls that provide neither.
        let term = if let Some(sq) = input.structured {
            crate::query::to_entrez(&sq).map_err(super::json_err)?
        } else if let Some(t) = input.term {
            t
        } else {
            return Err(ToolError::ValidationFailed {
                message: "pubmed_search requires `structured` or `term`".into(),
            });
        };

        let req = crate::types::ESearchRequest {
            db: "pubmed".into(),
            term,
            retmax: input.retmax,
            retstart: input.retstart,
            sort: input.sort,
            usehistory: Some(true),
            web_env: None,
            query_key: None,
            datetype: input.datetype,
            reldate: input.reldate,
            mindate: input.mindate,
            maxdate: input.maxdate,
        };

        let result = self.client.esearch(&req).await.map_err(super::json_err)?;

        let mut map = serde_json::Map::new();
        map.insert(
            "count".into(),
            serde_json::Value::String(result.result.count),
        );
        map.insert(
            "id_list".into(),
            serde_json::to_value(&result.result.id_list).unwrap_or_default(),
        );
        if let Some(ref web_env) = result.result.web_env {
            map.insert("web_env".into(), serde_json::Value::String(web_env.clone()));
        }
        if let Some(ref query_key) = result.result.query_key {
            map.insert(
                "query_key".into(),
                serde_json::Value::String(query_key.clone()),
            );
        }
        if let Some(ref qt) = result.result.query_translation {
            map.insert(
                "query_translation".into(),
                serde_json::Value::String(qt.clone()),
            );
        }

        Ok(AgentToolResult::success(format_esearch(
            &serde_json::Value::Object(map),
        )))
    }
}
