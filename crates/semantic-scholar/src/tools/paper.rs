use std::sync::Arc;

use agentik_core::tools::{ToolError, ToolFunction};
use agentik_proc::tool;
use agentik_sdk::types::ToolResult as AgentToolResult;
use async_trait::async_trait;

use crate::{S2Client, format::format_paper};

#[tool(
    name = "s2_paper",
    description = "Get detailed metadata for a single Semantic Scholar paper by ID. \
                  Returns full metadata including abstract, authors with IDs, \
                  citation/reference counts, external IDs (DOI, ArXiv, PubMed), \
                  publication venue, journal info, TLDR summary, and open access PDF link. \
                  \
                  Supported ID formats: \
                  - S2 paper SHA: '649def34f8be52c8b66281af98ae884c09aef38b' \
                  - CorpusId: 'CorpusId:215416146' \
                  - DOI: 'DOI:10.18653/v1/N18-3011' \
                  - ArXiv: 'ARXIV:2106.15928' \
                  - PubMed: 'PMID:19872477' \
                  - PubMed Central: 'PMCID:2323736' \
                  - URL: 'URL:https://arxiv.org/abs/2106.15928v1'"
)]
pub struct S2PaperInput {
    #[desc = "Paper identifier. Accepts S2 SHA, CorpusId:<id>, DOI:<doi>, \
             ARXIV:<id>, PMID:<id>, PMCID:<id>, or URL:<url>."]
    pub paper_id: String,
}

pub struct S2PaperTool {
    pub(crate) client: Arc<S2Client>,
}

#[async_trait]
impl ToolFunction for S2PaperTool {
    type Input = S2PaperInput;

    fn timeout_seconds(&self) -> u64 {
        60
    }

    async fn run(&self, input: Self::Input) -> Result<AgentToolResult, ToolError> {
        let paper = self
            .client
            .get_paper(&input.paper_id, None)
            .await
            .map_err(super::json_err)?;
        Ok(AgentToolResult::success(format_paper(&paper)))
    }
}
