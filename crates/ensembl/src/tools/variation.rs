use std::sync::Arc;

use agentik_core::tools::{ToolError, ToolFunction};
use agentik_proc::tool;
use agentik_sdk::types::ToolResult as AgentToolResult;
use async_trait::async_trait;

use super::json_err;
use crate::EnsemblClient;
use crate::format::format_variation;

#[tool(
    name = "ensembl_variation",
    description = "Get an Ensembl variation by rs/variant ID and summarize clinical significance, class, consequence, allele frequency, and genome mappings."
)]
pub struct EnsemblVariationInput {
    #[desc = "Species, e.g. 'human'."]
    pub species: String,

    #[desc = "Variation identifier, e.g. 'rs80357906'."]
    pub id: String,
}

pub struct EnsemblVariationTool {
    pub(crate) client: Arc<EnsemblClient>,
}

#[async_trait]
impl ToolFunction for EnsemblVariationTool {
    type Input = EnsemblVariationInput;

    async fn run(&self, input: Self::Input) -> Result<AgentToolResult, ToolError> {
        let variation = self
            .client
            .variation(&input.species, &input.id)
            .await
            .map_err(json_err)?;
        Ok(AgentToolResult::success(format_variation(&variation)))
    }
}
