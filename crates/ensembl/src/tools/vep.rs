use std::sync::Arc;

use agentik_core::tools::{ToolError, ToolFunction};
use agentik_proc::tool;
use agentik_sdk::types::ToolResult as AgentToolResult;
use async_trait::async_trait;

use super::json_err;
use crate::EnsemblClient;
use crate::format::format_vep;

#[tool(
    name = "ensembl_vep",
    description = "Annotate a variant with Ensembl VEP. Accept either a variant/rs ID or a region plus allele and summarize the most important transcript consequences."
)]
pub struct EnsemblVepInput {
    #[desc = "Species, e.g. 'human'."]
    pub species: String,

    #[desc = "Variant ID, e.g. 'rs80357906'. Omit when using region + allele."]
    pub id: Option<String>,

    #[desc = "Genomic region, e.g. '17:43057063-43057065'. Required with allele when id is omitted."]
    pub region: Option<String>,

    #[desc = "Alternate allele for a region request, e.g. 'G'. Required when id is omitted."]
    pub allele: Option<String>,
}

pub struct EnsemblVepTool {
    pub(crate) client: Arc<EnsemblClient>,
}

#[async_trait]
impl ToolFunction for EnsemblVepTool {
    type Input = EnsemblVepInput;

    async fn run(&self, input: Self::Input) -> Result<AgentToolResult, ToolError> {
        let results = if let Some(id) = input.id.as_deref() {
            self.client.vep_id(&input.species, id).await
        } else {
            let region = input
                .region
                .as_deref()
                .ok_or_else(|| ToolError::ValidationFailed {
                    message: "region and allele are required when id is omitted".to_string(),
                })?;
            let allele = input
                .allele
                .as_deref()
                .ok_or_else(|| ToolError::ValidationFailed {
                    message: "region and allele are required when id is omitted".to_string(),
                })?;
            self.client.vep_region(&input.species, region, allele).await
        }
        .map_err(json_err)?;
        Ok(AgentToolResult::success(format_vep(&results)))
    }
}
