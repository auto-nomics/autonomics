use std::sync::Arc;

use agentik_core::tools::{ToolError, ToolFunction};
use agentik_proc::tool;
use agentik_sdk::types::ToolResult;
use async_trait::async_trait;
use data_engine::runtime::DataEngineClient;

use crate::ExecError;

#[tool(
    name = "new_dag_ref",
    description = "Start a new independent analysis pipeline. Clears the current in-memory DAG \
                  (all nodes, edges, outputs) and switches the engine to a new history ref. \
                  Each ref maintains its own snapshot lineage — use this when starting an \
                  unrelated analysis task so its history stays separate from previous work."
)]
pub struct NewDagRefInput {
    /// Name for the new history ref (e.g. "gwas-bmi", "epi-charls").
    pub name: String,
}

pub struct NewDagRefTool {
    client: Arc<DataEngineClient>,
}

impl NewDagRefTool {
    pub fn new(client: Arc<DataEngineClient>) -> Self {
        Self { client }
    }
}

#[async_trait]
impl ToolFunction for NewDagRefTool {
    type Input = NewDagRefInput;

    async fn run(&self, input: Self::Input) -> Result<ToolResult, ToolError> {
        let name = input.name.clone();
        self.client
            .new_dag_ref(name.clone())
            .await
            .map_err(ExecError::from)?;
        Ok(ToolResult::success(format!(
            "DAG cleared, switched to ref '{name}'. Build your new pipeline from scratch."
        )))
    }
}
