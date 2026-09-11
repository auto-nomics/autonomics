use std::sync::Arc;

use agentik_core::tools::{ToolError, ToolFunction};
use agentik_proc::tool;
use agentik_sdk::types::ToolResult as AgentToolResult;
use async_trait::async_trait;

use crate::AlphaFoldClient;
use crate::format::format_predictions;

#[tool(
    name = "alphafold_lookup",
    description = "Look up AlphaFold predicted structure metadata by a UniProt accession. \
                  Returns model IDs, protein and organism context, model version, quality metric, \
                  sequence range, and CIF/PDB/confidence-file URLs."
)]
pub struct AlphaFoldLookupInput {
    #[desc = "UniProt accession, for example P01308 or A0A0B4J2F0."]
    pub accession: String,
}

pub struct AlphaFoldLookupTool {
    pub(crate) client: Arc<AlphaFoldClient>,
}

#[async_trait]
impl ToolFunction for AlphaFoldLookupTool {
    type Input = AlphaFoldLookupInput;

    fn timeout_seconds(&self) -> u64 {
        30
    }

    async fn run(&self, input: Self::Input) -> Result<AgentToolResult, ToolError> {
        let predictions = self
            .client
            .prediction(&input.accession)
            .await
            .map_err(|error| ToolError::ExecutionFailed {
                source: Box::new(error),
            })?;
        Ok(AgentToolResult::success(format_predictions(
            &predictions,
            &input.accession,
        )))
    }
}
