use std::sync::Arc;

use agentik_core::tools::{ToolError, ToolFunction};
use agentik_proc::tool;
use agentik_sdk::types::ToolResult as AgentToolResult;
use async_trait::async_trait;

use super::json_err;
use crate::ReactomeClient;
use crate::format::format_mapped_pathways;

#[tool(
    name = "reactome_mapping",
    description = "Map an external database identifier (e.g. a UniProt accession, \
                  Ensembl gene ID, or ChEBI compound ID) to Reactome pathways."
)]
pub struct MappingInput {
    #[desc = "External database name: 'UniProt', 'Ensembl', 'EntrezGene', 'ChEBI', etc."]
    pub resource: String,

    #[desc = "External identifier, e.g. 'P04637' (UniProt TP53)."]
    pub identifier: String,

    #[desc = "Set to true to map to reactions instead of pathways. Default false."]
    pub to_reactions: Option<bool>,
}

pub struct ReactomeMappingTool {
    pub(crate) client: Arc<ReactomeClient>,
}

#[async_trait]
impl ToolFunction for ReactomeMappingTool {
    type Input = MappingInput;

    async fn run(&self, input: Self::Input) -> Result<AgentToolResult, ToolError> {
        let pathways = if input.to_reactions.unwrap_or(false) {
            self.client
                .map_to_reactions(&input.resource, &input.identifier)
                .await
        } else {
            self.client
                .map_to_pathways(&input.resource, &input.identifier)
                .await
        }
        .map_err(json_err)?;
        Ok(AgentToolResult::success(format_mapped_pathways(
            &input.resource,
            &input.identifier,
            &pathways,
        )))
    }
}
