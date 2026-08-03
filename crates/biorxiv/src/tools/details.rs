use std::sync::Arc;

use agentik_core::tools::{ToolError, ToolFunction, ToolResult};
use agentik_proc::tool;
use agentik_sdk::types::ToolResult as AgentToolResult;
use async_trait::async_trait;

use crate::format::format_entries_full;
use crate::{BiorxivClient, types::Server};

#[tool(
    name = "biorxiv_details",
    description = "Fetch bioRxiv/medRxiv preprint metadata. Supports THREE lookup modes \
                  (provide exactly one): \
                  \
                  1. `doi` — fetch all versions of a single manuscript by its DOI \
                     (e.g. '10.1101/2024.01.15.573421'). \
                  \
                  2. `date_range` — browse papers posted within an inclusive date range \
                     ({\"from\": \"2024-01-01\", \"to\": \"2024-01-15\"}). \
                     Use `cursor` for pagination (100 per page). \
                  \
                  3. `recent` — fetch the N most recent papers from the server. \
                  \
                  Set `server` to 'medrxiv' (health sciences, default) or 'biorxiv' (biology). \
                  \
                  Returns: title, authors, corresponding author, abstract, category, \
                  version, license, and published-article DOI (if the preprint was \
                  published in a journal)."
)]
pub struct BiorxivDetailsInput {
    #[desc = "Server to query: 'medrxiv' (default) or 'biorxiv'."]
    pub server: Option<String>,

    #[desc = "Fetch by preprint DOI (e.g. '10.1101/2024.01.15.573421'). Returns all versions."]
    pub doi: Option<String>,

    #[desc = "Browse by date range: {\"from\": \"YYYY-MM-DD\", \"to\": \"YYYY-MM-DD\"}."]
    pub date_range: Option<DateRangeInput>,

    #[desc = "Fetch the N most recent papers (alternative to date_range)."]
    pub recent: Option<u32>,

    #[desc = "0-based page cursor for date_range queries (100 results per page)."]
    pub cursor: Option<u32>,
}

/// Date range input for the details tool.
#[derive(Debug, Clone, serde::Deserialize, serde::Serialize, schemars::JsonSchema)]
pub struct DateRangeInput {
    /// Start date (`YYYY-MM-DD`).
    pub from: String,
    /// End date (`YYYY-MM-DD`).
    pub to: String,
}

pub struct BiorxivDetailsTool {
    pub(crate) client: Arc<BiorxivClient>,
}

#[async_trait]
impl ToolFunction for BiorxivDetailsTool {
    type Input = BiorxivDetailsInput;

    fn timeout_seconds(&self) -> u64 {
        120
    }

    async fn run(&self, input: Self::Input) -> Result<AgentToolResult, ToolError> {
        let server: Server = input
            .server
            .as_deref()
            .unwrap_or("medrxiv")
            .parse()
            .map_err(|e: String| ToolError::ValidationFailed { message: e })?;

        let cursor = input.cursor.unwrap_or(0);

        // Mode 1: DOI lookup.
        if let Some(doi) = input.doi {
            let resp = self
                .client
                .details_by_doi(server, &doi)
                .await
                .map_err(super::json_err)?;

            if resp.collection.is_empty() {
                return Ok(AgentToolResult::success(
                    "No preprint found for that DOI.".to_string(),
                ));
            }

            return Ok(AgentToolResult::success(format_entries_full(
                &resp.collection,
            )));
        }

        // Mode 2: Date range browse.
        if let Some(dr) = input.date_range {
            let resp = self
                .client
                .details_by_date(
                    server,
                    chrono::NaiveDate::parse_from_str(&dr.from, "%Y-%m-%d")
                        .map_err(|e| ToolError::ValidationFailed {
                            message: format!("invalid 'from' date: {e}"),
                        })?,
                    chrono::NaiveDate::parse_from_str(&dr.to, "%Y-%m-%d")
                        .map_err(|e| ToolError::ValidationFailed {
                            message: format!("invalid 'to' date: {e}"),
                        })?,
                    cursor,
                )
                .await
                .map_err(super::json_err)?;

            return Ok(AgentToolResult::success(crate::format::format_details(
                &resp,
            )));
        }

        // Mode 3: Most recent.
        if let Some(n) = input.recent {
            let resp = self
                .client
                .details_recent(server, n)
                .await
                .map_err(super::json_err)?;

            return Ok(AgentToolResult::success(crate::format::format_details(
                &resp,
            )));
        }

        Err(ToolError::ValidationFailed {
            message: "biorxiv_details requires one of: `doi`, `date_range`, or `recent`".into(),
        })
    }
}
