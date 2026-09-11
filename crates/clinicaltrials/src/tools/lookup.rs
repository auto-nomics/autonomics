use std::sync::Arc;

use agentik_core::tools::{ToolError, ToolFunction};
use agentik_proc::tool;
use agentik_sdk::types::ToolResult as AgentToolResult;
use async_trait::async_trait;

use crate::ClinicalTrialsClient;
use crate::format::format_study;

#[tool(
    name = "clinicaltrials_study_lookup",
    description = "Fetch a ClinicalTrials.gov study by NCT ID. Returns status, sponsor, dates, \
                  conditions, design, interventions, primary outcomes, and the brief summary."
)]
pub struct ClinicalTrialsLookupInput {
    #[desc = "NCT identifier, for example NCT04280705."]
    pub nct_id: String,
}

pub struct ClinicalTrialsLookupTool {
    pub(crate) client: Arc<ClinicalTrialsClient>,
}

#[async_trait]
impl ToolFunction for ClinicalTrialsLookupTool {
    type Input = ClinicalTrialsLookupInput;

    fn timeout_seconds(&self) -> u64 {
        30
    }

    async fn run(&self, input: Self::Input) -> Result<AgentToolResult, ToolError> {
        let study =
            self.client
                .study(&input.nct_id)
                .await
                .map_err(|error| ToolError::ExecutionFailed {
                    source: Box::new(error),
                })?;
        Ok(AgentToolResult::success(format_study(&study)))
    }
}
