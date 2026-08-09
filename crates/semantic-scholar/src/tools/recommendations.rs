use std::sync::Arc;

use agentik_core::tools::{ToolError, ToolFunction};
use agentik_proc::tool;
use agentik_sdk::types::ToolResult as AgentToolResult;
use async_trait::async_trait;

use crate::{S2Client, format::format_recommendations};

#[tool(
    name = "s2_recommendations",
    description = "Get recommended papers similar to a given Semantic Scholar paper. \
                  Semantic Scholar's recommendation engine suggests papers that are \
                  topically related and likely relevant to the same research thread. \
                  \
                  Useful for literature discovery and finding related work. \
                  \
                  Paper ID formats: S2 SHA, CorpusId:<id>, DOI:<doi>, \
                  ARXIV:<id>, PMID:<id>, PMCID:<id>, URL:<url>."
)]
pub struct S2RecommendationsInput {
    #[desc = "Paper identifier to base recommendations on (S2 SHA, CorpusId:<id>, \
             DOI:<doi>, ARXIV:<id>, PMID:<id>, PMCID:<id>, or URL:<url>)."]
    pub paper_id: String,

    #[desc = "Maximum number of recommended papers to return (default 10, max 500)."]
    pub limit: Option<u32>,
}

pub struct S2RecommendationsTool {
    pub(crate) client: Arc<S2Client>,
}

#[async_trait]
impl ToolFunction for S2RecommendationsTool {
    type Input = S2RecommendationsInput;

    fn timeout_seconds(&self) -> u64 {
        60
    }

    async fn run(&self, input: Self::Input) -> Result<AgentToolResult, ToolError> {
        let resp = self
            .client
            .recommendations(&input.paper_id, input.limit.unwrap_or(10), None)
            .await
            .map_err(super::json_err)?;
        Ok(AgentToolResult::success(format_recommendations(&resp)))
    }
}
