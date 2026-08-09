use std::sync::Arc;

use agentik_core::tools::{ToolError, ToolFunction};
use agentik_proc::tool;
use agentik_sdk::types::ToolResult as AgentToolResult;
use async_trait::async_trait;

use crate::{S2Client, format::format_citations};

#[tool(
    name = "s2_citations",
    description = "Get papers that cite a given Semantic Scholar paper. \
                  Returns citing papers with metadata, citation context snippets, \
                  citation intents (methodology, background, result), and whether \
                  each citation is influential. \
                  \
                  Useful for tracking the impact and influence of a paper, \
                  and understanding how it is being used in subsequent research. \
                  \
                  Paper ID formats: S2 SHA, CorpusId:<id>, DOI:<doi>, \
                  ARXIV:<id>, PMID:<id>, PMCID:<id>, URL:<url>."
)]
pub struct S2CitationsInput {
    #[desc = "Paper identifier (S2 SHA, CorpusId:<id>, DOI:<doi>, ARXIV:<id>, \
             PMID:<id>, PMCID:<id>, or URL:<url>)."]
    pub paper_id: String,

    #[desc = "Maximum number of citations to return (default 20, max 1000)."]
    pub limit: Option<u32>,

    #[desc = "Offset for pagination (default 0)."]
    pub offset: Option<u32>,
}

pub struct S2CitationsTool {
    pub(crate) client: Arc<S2Client>,
}

#[async_trait]
impl ToolFunction for S2CitationsTool {
    type Input = S2CitationsInput;

    fn timeout_seconds(&self) -> u64 {
        120
    }

    async fn run(&self, input: Self::Input) -> Result<AgentToolResult, ToolError> {
        let resp = self
            .client
            .get_citations(
                &input.paper_id,
                input.limit.unwrap_or(20),
                input.offset.unwrap_or(0),
                None,
            )
            .await
            .map_err(super::json_err)?;
        Ok(AgentToolResult::success(format_citations(&resp)))
    }
}
