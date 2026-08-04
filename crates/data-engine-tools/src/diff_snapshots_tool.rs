use std::sync::Arc;

use agentik_core::tools::{ToolError, ToolFunction};
use agentik_proc::tool;
use agentik_sdk::types::ToolResult;
use async_trait::async_trait;
use data_engine::runtime::DataEngineClient;

use crate::ExecError;

#[tool(
    name = "diff_snapshots",
    description = "Compare two snapshots and show what changed between them: added/removed/modified \
                  nodes and added/removed edges. Use dag_history_log or show_snapshot to find \
                  snapshot ids. Short-hash prefixes are accepted."
)]
pub struct DiffSnapshotsInput {
    /// Old snapshot id or short-hash prefix.
    pub old_id: String,
    /// New snapshot id or short-hash prefix.
    pub new_id: String,
}

pub struct DiffSnapshotsTool {
    client: Arc<DataEngineClient>,
}

impl DiffSnapshotsTool {
    pub fn new(client: Arc<DataEngineClient>) -> Self {
        Self { client }
    }
}

#[async_trait]
impl ToolFunction for DiffSnapshotsTool {
    type Input = DiffSnapshotsInput;

    async fn run(&self, input: Self::Input) -> Result<ToolResult, ToolError> {
        let diff = self
            .client
            .diff_snapshots(input.old_id.clone(), input.new_id.clone())
            .await
            .map_err(ExecError::from)?;

        Ok(ToolResult::success(diff))
    }
}
