use std::sync::Arc;

use agentik_core::tools::{ToolError, ToolFunction};
use agentik_proc::tool;
use agentik_sdk::types::ToolResult;
use async_trait::async_trait;
use data_engine::runtime::DataEngineClient;

use crate::ExecError;

#[tool(
    name = "branch_from_snapshot",
    description = "Create a new history ref diverging from an arbitrary snapshot, switch the \
                  engine to it, and load that snapshot's DAG. This is like `git checkout -b \
                  <branch> <commit>` — you start a new independent lineage from a historical \
                  point without affecting the original ref. Short-hash prefixes are accepted."
)]
pub struct BranchFromSnapshotInput {
    /// Snapshot id or short-hash prefix to branch from.
    pub snapshot_id: String,
    /// Name for the new ref.
    pub ref_name: String,
}

pub struct BranchFromSnapshotTool {
    client: Arc<DataEngineClient>,
}

impl BranchFromSnapshotTool {
    pub fn new(client: Arc<DataEngineClient>) -> Self {
        Self { client }
    }
}

#[async_trait]
impl ToolFunction for BranchFromSnapshotTool {
    type Input = BranchFromSnapshotInput;

    async fn run(&self, input: Self::Input) -> Result<ToolResult, ToolError> {
        let snapshot_id = input.snapshot_id.clone();
        let ref_name = input.ref_name.clone();
        self.client
            .branch_from_snapshot(snapshot_id.clone(), ref_name.clone())
            .await
            .map_err(ExecError::from)?;
        Ok(ToolResult::success(format!(
            "Created ref '{ref_name}' from snapshot '{snapshot_id}' and loaded its DAG. \
             Subsequent run_dag calls commit to '{ref_name}'."
        )))
    }
}
