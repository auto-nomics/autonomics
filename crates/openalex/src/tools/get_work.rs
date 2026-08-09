use std::sync::Arc;

use agentik_core::tools::{ToolError, ToolFunction};
use agentik_proc::tool;
use agentik_sdk::types::ToolResult as AgentToolResult;
use async_trait::async_trait;

use crate::format::format_work_detail;
use crate::OpenAlexClient;

#[tool(
    name = "openalex_get_work",
    description = "Retrieve a single work from OpenAlex by ID with full detail \
                  (abstract, all authors with affiliations, all locations, MeSH terms, \
                  keywords, citation counts by year, references). \
                  \
                  Accepted ID formats: \
                  - OpenAlex ID: 'W2741809807' \
                  - DOI (full URL): 'https://doi.org/10.7717/peerj.4375' \
                  - DOI (shortcut): 'doi:10.7717/peerj.4375' \
                  - PMID (shortcut): 'pmid:29456894'"
)]
pub struct OpenAlexGetWorkInput {
    #[desc = "Work identifier (OpenAlex ID, DOI, or PMID). See tool description for formats."]
    pub id: String,
}

pub struct OpenAlexGetWorkTool {
    pub(crate) client: Arc<OpenAlexClient>,
}

#[async_trait]
impl ToolFunction for OpenAlexGetWorkTool {
    type Input = OpenAlexGetWorkInput;

    fn timeout_seconds(&self) -> u64 {
        60
    }

    async fn run(&self, input: Self::Input) -> Result<AgentToolResult, ToolError> {
        let work = self.client.get_work(&input.id).await.map_err(super::json_err)?;
        Ok(AgentToolResult::success(format_work_detail(&work)))
    }
}
