use std::sync::Arc;

use agentik_core::tools::{ToolError, ToolFunction};
use agentik_proc::tool;
use agentik_sdk::types::ToolResult;
use async_trait::async_trait;
use data_engine::runtime::DataEngineClient;

use crate::ExecError;

#[tool(
    name = "switch_dag_ref",
    description = "Switch the engine's active history ref to an existing one. \
                  Subsequent run_dag calls will commit snapshots to the new ref. \
                  The in-memory DAG is NOT cleared — use new_dag_ref if you want \
                  a fresh start."
)]
pub struct SwitchDagRefInput {
    /// Name of the ref to switch to.
    pub name: String,
}

pub struct SwitchDagRefTool {
    client: Arc<DataEngineClient>,
}

impl SwitchDagRefTool {
    pub fn new(client: Arc<DataEngineClient>) -> Self {
        Self { client }
    }
}

#[async_trait]
impl ToolFunction for SwitchDagRefTool {
    type Input = SwitchDagRefInput;

    async fn run(&self, input: Self::Input) -> Result<ToolResult, ToolError> {
        self.client
            .switch_dag_ref(input.name.clone())
            .await
            .map_err(ExecError::from)?;
        Ok(ToolResult::success(format!(
            "Switched to ref '{}'",
            input.name
        )))
    }
}
