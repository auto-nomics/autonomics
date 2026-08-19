use std::sync::Arc;

use agentik_sdk::types::ToolResult;
use async_trait::async_trait;
use data_engine::runtime::DataEngineClient;

use agentik_core::tools::{ToolError, ToolFunction};
use agentik_proc::tool;

use crate::ExecError;

#[tool(
    name = "remove_node",
    description = "Remove a node from the DAG. Fails if the node has downstream \
                  dependents (nodes that rely on its output). Remove those \
                  incoming edges with `remove_edge`, or remove the dependents \
                  first."
)]
pub struct RemoveNodeInput {
    #[desc = "ID of the node to remove from the DAG"]
    pub id: String,
}

pub struct RemoveNodeTool {
    client: Arc<DataEngineClient>,
}

impl RemoveNodeTool {
    pub fn new(client: Arc<DataEngineClient>) -> Self {
        Self { client }
    }
}

#[async_trait]
impl ToolFunction for RemoveNodeTool {
    type Input = RemoveNodeInput;

    async fn run(&self, input: Self::Input) -> Result<ToolResult, ToolError> {
        let msg = format!("node '{}' removed from DAG", input.id);

        self.client
            .remove_node(input.id)
            .await
            .map_err(ExecError::from)?;

        Ok(ToolResult::success(msg))
    }
}
