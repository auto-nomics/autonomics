use std::sync::Arc;

use agentik_core::tools::{ToolError, ToolFunction};
use agentik_proc::tool;
use agentik_sdk::types::ToolResult as AgentToolResult;
use async_trait::async_trait;

use super::json_err;
use crate::ReactomeClient;
use crate::format::format_species;

#[tool(
    name = "reactome_species",
    description = "List Reactome species. Use `main=true` for the default subset \
                  (human, mouse, rat, etc.) or `main=false` for all curated species."
)]
pub struct SpeciesInput {
    #[desc = "true = main species only (default); false = all species."]
    pub main: Option<bool>,
}

pub struct ReactomeSpeciesTool {
    pub(crate) client: Arc<ReactomeClient>,
}

#[async_trait]
impl ToolFunction for ReactomeSpeciesTool {
    type Input = SpeciesInput;

    async fn run(&self, input: Self::Input) -> Result<AgentToolResult, ToolError> {
        let species = if input.main.unwrap_or(true) {
            self.client.species_main().await
        } else {
            self.client.species_all().await
        }
        .map_err(json_err)?;
        Ok(AgentToolResult::success(format_species(&species)))
    }
}
