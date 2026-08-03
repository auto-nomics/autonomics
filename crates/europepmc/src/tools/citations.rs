use std::sync::Arc;

use agentik_core::tools::{ToolError, ToolFunction, ToolResult};
use agentik_proc::tool;
use agentik_sdk::types::ToolResult as AgentToolResult;
use async_trait::async_trait;

use crate::{EuropePmcClient, format::format_citations};

#[tool(
    name = "europepmc_citations",
    description = "Retrieve the citation list (works that cite) a given publication \
                  in Europe PMC. Returns a count and list of citing publications \
                  with metadata (title, authors, journal, year, citation count). \
                  Useful for measuring impact and finding follow-up research. \
                  \
                  Example: source='MED', id='29867326'"
)]
pub struct EuropePmcCitationsInput {
    #[desc = "Three-letter source code of the article (e.g. 'MED', 'PMC'). Default: 'MED'."]
    pub source: Option<String>,

    #[desc = "Publication identifier (e.g. PubMed ID '29867326')."]
    pub id: String,

    #[desc = "Page number (1-based, default 1)."]
    pub page: Option<u32>,

    #[desc = "Results per page (default 25, max 1000)."]
    pub page_size: Option<u32>,
}

pub struct EuropePmcCitationsTool {
    pub(crate) client: Arc<EuropePmcClient>,
}

#[async_trait]
impl ToolFunction for EuropePmcCitationsTool {
    type Input = EuropePmcCitationsInput;

    fn timeout_seconds(&self) -> u64 {
        120
    }

    async fn run(&self, input: Self::Input) -> Result<AgentToolResult, ToolError> {
        let source = input.source.as_deref().unwrap_or("MED");

        let resp = self
            .client
            .citations(
                source,
                &input.id,
                crate::types::PageParams {
                    page: input.page,
                    page_size: input.page_size,
                },
            )
            .await
            .map_err(super::json_err)?;

        Ok(AgentToolResult::success(format_citations(&resp)))
    }
}
