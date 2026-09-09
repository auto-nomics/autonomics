use std::sync::Arc;

use agentik_core::tools::{ToolError, ToolFunction};
use agentik_proc::tool;
use agentik_sdk::types::ToolResult;
use async_trait::async_trait;

use crate::RcsbClient;
use crate::format::format_polymer_entity;

#[tool(
    name = "rcsb_polymer",
    description = "Fetch a polymer entity from an RCSB PDB structure and summarize its description, sequence, length, type, copy count, UniProt mapping, source organism, genes, and available annotations."
)]
pub struct RcsbPolymerInput {
    #[desc = "Four-character PDB entry ID, e.g. '4HHB'."]
    pub entry_id: String,

    #[desc = "Polymer entity ID, usually 1, 2, and so on. Use 0 to fetch the first entity."]
    pub entity_id: u32,
}

pub struct RcsbPolymerTool {
    pub client: Arc<RcsbClient>,
}

#[async_trait]
impl ToolFunction for RcsbPolymerTool {
    type Input = RcsbPolymerInput;

    fn timeout_seconds(&self) -> u64 {
        120
    }

    async fn run(&self, input: Self::Input) -> Result<ToolResult, ToolError> {
        let entity = if input.entity_id == 0 {
            let mut entities = self
                .client
                .polymer_entities_for_entry(&input.entry_id)
                .await
                .map_err(super::tool_error)?;
            entities.pop().ok_or_else(|| ToolError::ExecutionFailed {
                source: Box::new(crate::RcsbError::Param(format!(
                    "entry {} has no polymer entities",
                    input.entry_id.to_ascii_uppercase()
                ))),
            })?
        } else {
            self.client
                .polymer_entity(&input.entry_id, input.entity_id)
                .await
                .map_err(super::tool_error)?
        };

        Ok(ToolResult::success(format_polymer_entity(&entity)))
    }
}
