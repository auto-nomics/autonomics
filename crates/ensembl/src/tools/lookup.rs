use std::sync::Arc;

use agentik_core::tools::{ToolError, ToolFunction};
use agentik_proc::tool;
use agentik_sdk::types::ToolResult as AgentToolResult;
use async_trait::async_trait;

use super::json_err;
use crate::EnsemblClient;
use crate::format::format_lookup;

#[tool(
    name = "ensembl_lookup",
    description = "Resolve a stable Ensembl gene, transcript, translation, exon, or regulatory feature ID and preview its coordinates, biotype, description, and optional transcripts."
)]
pub struct EnsemblLookupInput {
    #[desc = "Stable Ensembl ID, e.g. 'ENSG00000139618' or 'ENST00000380152'."]
    pub id: String,

    #[desc = "Expand child transcript, translation, and exon records for genes. Default false."]
    pub expand: Option<bool>,

    #[desc = "Optional species, e.g. 'human'. Supply it only when an ID is ambiguous."]
    pub species: Option<String>,
}

pub struct EnsemblLookupTool {
    pub(crate) client: Arc<EnsemblClient>,
}

#[async_trait]
impl ToolFunction for EnsemblLookupTool {
    type Input = EnsemblLookupInput;

    async fn run(&self, input: Self::Input) -> Result<AgentToolResult, ToolError> {
        let entry = if let Some(species) = input.species.as_deref() {
            self.client
                .lookup_id_with_species(species, &input.id, input.expand.unwrap_or(false))
                .await
        } else {
            self.client
                .lookup_id(&input.id, input.expand.unwrap_or(false))
                .await
        }
        .map_err(json_err)?;
        Ok(AgentToolResult::success(format_lookup(&entry)))
    }
}
