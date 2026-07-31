use std::sync::Arc;

use agentik_core::tools::{ToolError, ToolFunction, ToolResult};
use agentik_proc::tool;
use agentik_sdk::types::ToolResult as AgentToolResult;
use async_trait::async_trait;

use crate::{EutilsClient, format::format_esearch};

#[tool(
    name = "pubmed_search",
    description = "Search PubMed with Entrez query syntax. Returns matching PMIDs, \
                  total count, and history info for chaining into pubmed_fetch or \
                  pubmed_summary. Supports date filters, sort orders, and pagination. \
                  \
                  The `term` MUST use structured Entrez query syntax, NOT plain keywords. \
                  Always qualify search terms with field tags for precise results. \
                  \
                  Field tags: [Title/Abstract], [Title], [Author], [MeSH], \
                  [Publication Type], [Journal], [Affiliation], [Year]. \
                  \
                  Boolean operators (uppercase): AND, OR, NOT. \
                  Parentheses group sub-expressions. \
                  \
                  Examples: \
                  • \"cancer immunotherapy[Title/Abstract]\" \
                  • \"CRISPR[Title/Abstract] AND review[Publication Type]\" \
                  • \"(cancer OR neoplasm)[Title] AND Smith J[Author]\" \
                  • \"brca1[Title/Abstract] AND 2020:2024[Year]"
)]
pub struct PubmedSearchInput {
    #[desc = "Entrez query expression. Use field tags and boolean operators (see tool \
             description). Do NOT send plain space-separated keywords. \
             Example: '(cancer OR tumor)[Title/Abstract] AND review[Publication Type]'"]
    pub term: String,
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
        let req = crate::types::ESearchRequest {
            db: "pubmed".into(),
            term: input.term,
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
