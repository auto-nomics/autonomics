use std::sync::Arc;

use agentik_core::tools::{ToolError, ToolFunction};
use agentik_proc::tool;
use agentik_sdk::types::ToolResult;
use async_trait::async_trait;

use crate::RcsbClient;
use crate::format::format_entry;

#[tool(
    name = "rcsb_entry",
    description = "Fetch one RCSB PDB entry by four-character ID and return a structured summary: title, experiment, resolution, atom and entity counts, molecular weight, symmetry, release dates, authors, and primary citation."
)]
pub struct RcsbEntryInput {
    #[desc = "Four-character PDB entry ID, e.g. '4HHB'."]
    pub entry_id: String,
}

pub struct RcsbEntryTool {
    pub client: Arc<RcsbClient>,
}

#[async_trait]
impl ToolFunction for RcsbEntryTool {
    type Input = RcsbEntryInput;

    fn timeout_seconds(&self) -> u64 {
        120
    }

    async fn run(&self, input: Self::Input) -> Result<ToolResult, ToolError> {
        let entry = self
            .client
            .entry(&input.entry_id)
            .await
            .map_err(super::tool_error)?;
        Ok(ToolResult::success(format_entry(&entry)))
    }
}
