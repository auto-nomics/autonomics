use std::sync::Arc;

use agentik_core::tools::{ToolError, ToolFunction};
use agentik_proc::tool;
use agentik_sdk::types::ToolResult;
use async_trait::async_trait;
use data_engine::runtime::DataEngineClient;

use crate::ExecError;

#[tool(
    name = "checkout_dag",
    description = "Load a historical snapshot's DAG into memory without moving the history ref. \
                  This is like `git checkout <commit>` — you inspect and work from a past state, \
                  but the ref stays where it is. Subsequent run_dag calls commit with the current \
                  ref head as parent. Short-hash prefixes are accepted. \
                  Use dag_history_log to find snapshot ids."
)]
pub struct CheckoutDagInput {
    /// Snapshot id or short-hash prefix to load.
    pub snapshot_id: String,
}

pub struct CheckoutDagTool {
    client: Arc<DataEngineClient>,
}

impl CheckoutDagTool {
    pub fn new(client: Arc<DataEngineClient>) -> Self {
        Self { client }
    }
}

#[async_trait]
impl ToolFunction for CheckoutDagTool {
    type Input = CheckoutDagInput;

    async fn run(&self, input: Self::Input) -> Result<ToolResult, ToolError> {
        self.client
            .checkout_dag(input.snapshot_id.clone())
            .await
            .map_err(ExecError::from)?;
        Ok(ToolResult::success(format!(
            "DAG loaded from snapshot '{}'. The history ref is unchanged — \
             use run_dag to commit from this state, or branch_from_snapshot \
             to start a new lineage.",
            input.snapshot_id
        )))
    }
}
