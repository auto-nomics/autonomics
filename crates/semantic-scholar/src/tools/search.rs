use std::sync::Arc;

use agentik_core::tools::{ToolError, ToolFunction};
use agentik_proc::tool;
use agentik_sdk::types::ToolResult as AgentToolResult;
use async_trait::async_trait;

use crate::{S2Client, format::format_search};

use bib_types::StructuredSearch;

#[tool(
    name = "s2_search",
    description = "Search Semantic Scholar (214M+ papers across all fields) and return \
                  matching papers with metadata including title, abstract, authors, \
                  citation counts, TLDR summaries, and more. \
                  \
                  TWO query modes (provide exactly one): \
                  \
                  1. RECOMMENDED — `structured`: a typed query object with named fields \
                     (keywords, title, authors, journal, publication_types, year_range). \
                     The tool translates it to the Semantic Scholar query + filters for you. \
                     \
                     Example: { \
                       \"keywords\": [\"p53\", \"cancer\"], \
                       \"year_range\": {\"from\": 2020, \"to\": 2024} \
                     } \
                  \
                  2. EXPERT — `query`: a plain-text query string. Semantic Scholar matches \
                     it against the paper's title and abstract. No special query syntax \
                     is supported for relevance search (use the `bulk` param for boolean queries). \
                     \
                     Example: 'covid vaccination efficacy' \
                  \
                  Use `bulk: true` for the bulk search endpoint which supports boolean \
                  query syntax (+, |, -, \"\", *, ~N) and returns up to 10M results \
                  via continuation tokens."
)]
pub struct S2SearchInput {
    #[desc = "Structured query object. Preferred over `query`. Populate any subset of \
             fields: keywords, title, authors, journal, publication_types, year_range. \
             Text fields are space-joined (implicit AND); filters map to S2 query params."]
    pub structured: Option<StructuredSearch>,

    #[desc = "Raw plain-text query string (expert mode). Used only when \
             `structured` is absent. Example: 'covid vaccination efficacy'."]
    pub query: Option<String>,

    #[desc = "Maximum number of results to return (default 10, max 100 for relevance \
             search, 1000 for bulk)."]
    pub limit: Option<u32>,

    #[desc = "Use the bulk search endpoint, which supports boolean query syntax \
             (+, |, -, \"\", *, ~N) and continuation tokens for large result sets. \
             Default false."]
    pub bulk: Option<bool>,

    #[desc = "Continuation token from a previous bulk search response, to fetch \
             the next batch of results."]
    pub token: Option<String>,
}

pub struct S2SearchTool {
    pub(crate) client: Arc<S2Client>,
}

#[async_trait]
impl ToolFunction for S2SearchTool {
    type Input = S2SearchInput;

    fn timeout_seconds(&self) -> u64 {
        120
    }

    async fn run(&self, input: Self::Input) -> Result<AgentToolResult, ToolError> {
        let limit = input.limit.unwrap_or(10);

        if input.bulk.unwrap_or(false) {
            // ---- Bulk search ----
            let query = if let Some(sq) = input.structured {
                crate::query::to_s2(&sq).map_err(super::json_err)?.query
            } else if let Some(q) = input.query {
                q
            } else {
                return Err(ToolError::ValidationFailed {
                    message: "s2_search requires `structured` or `query`".into(),
                });
            };
            let resp = self
                .client
                .search_paper_bulk(
                    &query,
                    input.token.as_deref(),
                    None,
                    None,
                )
                .await
                .map_err(super::json_err)?;

            let mut out = String::with_capacity(8192);
            out.push_str(&format!("**{}** total results (bulk)\n\n", resp.total));
            for (i, paper) in resp.data.iter().enumerate() {
                crate::format::format_paper_preview(i + 1, paper, &mut out);
                out.push('\n');
            }
            if let Some(ref t) = resp.token {
                out.push_str(&format!(
                    "_Continuation token:_ `{t}`\n\nPass this as `token` to fetch the next batch.\n"
                ));
            }
            return Ok(AgentToolResult::success(out.trim_end().to_string()));
        }

        // ---- Relevance search ----
        let resp = if let Some(sq) = input.structured {
            let parts = crate::query::to_s2(&sq).map_err(super::json_err)?;
            self.client
                .search_paper_filtered(&parts.query, limit, 0, &parts.filter, None)
                .await
                .map_err(super::json_err)?
        } else if let Some(q) = input.query {
            self.client
                .search_paper(&q, limit, None)
                .await
                .map_err(super::json_err)?
        } else {
            return Err(ToolError::ValidationFailed {
                message: "s2_search requires `structured` or `query`".into(),
            });
        };

        Ok(AgentToolResult::success(format_search(&resp)))
    }
}
