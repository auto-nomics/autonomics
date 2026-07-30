use std::sync::Arc;

use agentik_core::tools::{ToolError, ToolFunction};
use agentik_proc::tool;
use agentik_sdk::types::ToolResult as AgentToolResult;
use async_trait::async_trait;

use super::json_err;
use crate::format::format_study;
use crate::OpenTargetsClient;

#[tool(
    name = "opentargets_study",
    description = "Get the annotation card for a GWAS study by study ID. Returns trait, \
                  study type, PMID, journal, sample sizes (cases/controls), and whether \
                  summary statistics are available."
)]
pub struct StudyInput {
    #[desc = "Study ID, e.g. 'GCST006131' (GWAS Catalog) or a FinnGen / UKB-b study ID."]
    pub study_id: String,
}

pub struct StudyTool {
    pub(crate) client: Arc<OpenTargetsClient>,
}

#[async_trait]
impl ToolFunction for StudyTool {
    type Input = StudyInput;

    async fn run(&self, input: Self::Input) -> Result<AgentToolResult, ToolError> {
        let study = self.client.study(&input.study_id).await.map_err(json_err)?;
        match study {
            Some(s) => Ok(AgentToolResult::success(format_study(&s))),
            None => Ok(AgentToolResult::error(format!(
                "No study found for ID '{}'.",
                input.study_id
            ))),
        }
    }
}
