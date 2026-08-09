use std::sync::Arc;

use agentik_core::tools::{ToolError, ToolFunction};
use agentik_proc::tool;
use agentik_sdk::types::ToolResult as AgentToolResult;
use async_trait::async_trait;

use crate::{S2Client, format::format_references};

#[tool(
    name = "s2_references",
    description = "Get papers referenced by (cited in the bibliography of) a given \
                  Semantic Scholar paper. Returns referenced papers with metadata, \
                  citation context snippets, and intents. \
                  \
                  Useful for exploring the intellectual foundations of a paper \
                  and finding related prior work. \
                  \
                  Paper ID formats: S2 SHA, CorpusId:<id>, DOI:<doi>, \
                  ARXIV:<id>, PMID:<id>, PMCID:<id>, URL:<url>."
)]
pub struct S2ReferencesInput {
    #[desc = "Paper identifier (S2 SHA, CorpusId:<id>, DOI:<doi>, ARXIV:<id>, \
             PMID:<id>, PMCID:<id>, or URL:<url>)."]
    pub paper_id: String,

    #[desc = "Maximum number of references to return (default 20, max 1000)."]
    pub limit: Option<u32>,

    #[desc = "Offset for pagination (default 0)."]
    pub offset: Option<u32>,
}

pub struct S2ReferencesTool {
    pub(crate) client: Arc<S2Client>,
}

#[async_trait]
impl ToolFunction for S2ReferencesTool {
    type Input = S2ReferencesInput;

    fn timeout_seconds(&self) -> u64 {
        120
    }

    async fn run(&self, input: Self::Input) -> Result<AgentToolResult, ToolError> {
        let resp = self
            .client
            .get_references(
                &input.paper_id,
                input.limit.unwrap_or(20),
                input.offset.unwrap_or(0),
                None,
            )
            .await
            .map_err(super::json_err)?;
        Ok(AgentToolResult::success(format_references(&resp)))
    }
}
