use std::sync::Arc;

use agentik_core::tools::{ToolError, ToolFunction};
use agentik_proc::tool;
use agentik_sdk::types::ToolResult;
use async_trait::async_trait;
use data_engine::runtime::DataEngineClient;

use crate::ExecError;

#[tool(
    name = "list_dag_refs",
    description = "List all history refs (branches) and the snapshot each currently \
                  points to. Use this to see which independent analysis lineages \
                  exist and choose which one to switch to."
)]
pub struct ListDagRefsInput {}

pub struct ListDagRefsTool {
    client: Arc<DataEngineClient>,
}

impl ListDagRefsTool {
    pub fn new(client: Arc<DataEngineClient>) -> Self {
        Self { client }
    }
}

#[async_trait]
impl ToolFunction for ListDagRefsTool {
    type Input = ListDagRefsInput;

    async fn run(&self, _input: Self::Input) -> Result<ToolResult, ToolError> {
        let refs = self.client.list_dag_refs().await.map_err(ExecError::from)?;

        if refs.is_empty() {
            return Ok(ToolResult::success("No history refs found (no history store attached)."));
        }

        let current_ref = self.client.get_dag_ref().await.map_err(ExecError::from)?;

        let mut out = String::new();
        for (name, snap_id, pinned) in &refs {
            let short_id = &snap_id[..12.min(snap_id.len())];
            let marker = if name == &current_ref { " * " } else { "   " };
            let ty = if *pinned { "tag" } else { "branch" };
            out.push_str(&format!("{marker}{name:<24} {short_id}  ({ty})\n"));
        }
        out.push_str("\n* = current active ref");

        Ok(ToolResult::success(out))
    }
}
