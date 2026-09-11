use std::sync::Arc;

use agentik_core::tools::{ToolError, ToolFunction};
use agentik_proc::tool;
use agentik_sdk::types::ToolResult as AgentToolResult;
use async_trait::async_trait;

use crate::PubChemClient;
use crate::format::format_compound;
use crate::types::CompoundIdentifierType;

#[tool(
    name = "pubchem_compound_lookup",
    description = "Look up a compound in PubChem by name, CID, or InChIKey. Returns CID, molecular \
                  formula and weight, IUPAC name, SMILES, InChI, and InChIKey."
)]
pub struct PubChemLookupInput {
    #[desc = "Identifier type: name, cid, or inchikey."]
    pub identifier_type: String,

    #[desc = "Compound identifier, for example aspirin, 2244, or an InChIKey."]
    pub identifier: String,
}

pub struct PubChemLookupTool {
    pub(crate) client: Arc<PubChemClient>,
}

#[async_trait]
impl ToolFunction for PubChemLookupTool {
    type Input = PubChemLookupInput;

    fn timeout_seconds(&self) -> u64 {
        30
    }

    async fn run(&self, input: Self::Input) -> Result<AgentToolResult, ToolError> {
        let identifier_type =
            CompoundIdentifierType::parse(&input.identifier_type).ok_or_else(|| {
                ToolError::ValidationFailed {
                    message: format!(
                        "identifier_type must be name, cid, or inchikey; got {}",
                        input.identifier_type
                    ),
                }
            })?;
        let compound = self
            .client
            .compound(identifier_type, &input.identifier)
            .await
            .map_err(|error| ToolError::ExecutionFailed {
                source: Box::new(error),
            })?;
        match compound {
            Some(compound) => Ok(AgentToolResult::success(format_compound(
                &compound,
                &input.identifier,
            ))),
            None => Ok(AgentToolResult::error(format!(
                "No PubChem compound was found for {} `{}`.",
                identifier_type.as_str(),
                input.identifier
            ))),
        }
    }
}
