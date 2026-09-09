use std::sync::Arc;

use agentik_core::tools::{ToolError, ToolFunction};
use agentik_proc::tool;
use agentik_sdk::types::ToolResult as AgentToolResult;
use async_trait::async_trait;

use super::json_err;
use crate::ReactomeClient;
use crate::format::format_analysis;

#[tool(
    name = "reactome_analysis",
    description = "Submit a list of gene/protein identifiers for Reactome pathway \
                  over-representation analysis. Returns significantly enriched \
                  pathways with p-values and FDR."
)]
pub struct AnalysisInput {
    #[desc = "Gene or protein identifiers to analyse, e.g. ['TP53', 'BRCA1', 'EGFR']. \
              UniProt accessions, Ensembl IDs, Entrez Gene IDs, or HGNC symbols are accepted."]
    pub identifiers: Vec<String>,

    #[desc = "Project orthologs to human. Default true."]
    pub project_to_human: Option<bool>,
}

pub struct ReactomeAnalysisTool {
    pub(crate) client: Arc<ReactomeClient>,
}

#[async_trait]
impl ToolFunction for ReactomeAnalysisTool {
    type Input = AnalysisInput;

    fn timeout_seconds(&self) -> u64 {
        120
    }

    async fn run(&self, input: Self::Input) -> Result<AgentToolResult, ToolError> {
        let result = self
            .client
            .analyse_identifiers(
                &input.identifiers,
                input.project_to_human.unwrap_or(true),
            )
            .await
            .map_err(json_err)?;
        Ok(AgentToolResult::success(format_analysis(&result)))
    }
}
