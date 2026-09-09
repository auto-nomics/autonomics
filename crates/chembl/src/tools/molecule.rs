use std::sync::Arc;

use agentik_core::tools::{ToolError, ToolFunction};
use agentik_proc::tool;
use agentik_sdk::types::ToolResult as AgentToolResult;
use async_trait::async_trait;

use super::json_err;
use crate::{ChEMBLClient, format::format_molecule};

#[tool(
    name = "chembl_molecule_summary",
    description = "Get a compound information card from ChEMBL by ChEMBL ID, including \
                  identity, drug development phase, calculated properties, structural \
                  identifiers, and synonyms."
)]
pub struct MoleculeInput {
    #[desc = "ChEMBL molecule ID, e.g. 'CHEMBL25'."]
    pub chembl_id: String,
}

pub struct MoleculeTool {
    pub(crate) client: Arc<ChEMBLClient>,
}

#[async_trait]
impl ToolFunction for MoleculeTool {
    type Input = MoleculeInput;

    async fn run(&self, input: Self::Input) -> Result<AgentToolResult, ToolError> {
        match self
            .client
            .molecule(&input.chembl_id)
            .await
            .map_err(json_err)?
        {
            Some(molecule) => Ok(AgentToolResult::success(format_molecule(&molecule))),
            None => Ok(AgentToolResult::error(format!(
                "No ChEMBL molecule found for '{}'.",
                input.chembl_id
            ))),
        }
    }
}
