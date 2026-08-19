use std::sync::Arc;

use agentik_core::tools::{ToolError, ToolFunction};
use agentik_proc::tool;
use agentik_sdk::types::ToolResult;
use async_trait::async_trait;
use data_engine::runtime::DataEngineClient;

use crate::ExecError;

#[tool(
    name = "remove_edge",
    description = "Remove one DAG edge identified by its endpoint node IDs and \
                   ports. The target node and its descendants become dirty. \
                   Use this before deleting an upstream node that still has \
                   downstream dependents."
)]
pub struct RemoveEdgeInput {
    #[desc = "ID of the upstream (source) node"]
    pub from: String,
    #[desc = "Output port index on the 'from' node"]
    pub from_port: u8,
    #[desc = "ID of the downstream (target) node"]
    pub to: String,
    #[desc = "Input port index on the 'to' node"]
    pub to_port: u8,
}

pub struct RemoveEdgeTool {
    client: Arc<DataEngineClient>,
}

impl RemoveEdgeTool {
    pub fn new(client: Arc<DataEngineClient>) -> Self {
        Self { client }
    }
}

#[async_trait]
impl ToolFunction for RemoveEdgeTool {
    type Input = RemoveEdgeInput;

    async fn run(&self, input: Self::Input) -> Result<ToolResult, ToolError> {
        let msg = format!(
            "edge removed: {}.{} -> {}.{}",
            input.from, input.from_port, input.to, input.to_port
        );

        self.client
            .delete_edge(input.from, input.from_port, input.to, input.to_port)
            .await
            .map_err(ExecError::from)?;

        Ok(ToolResult::success(msg))
    }
}
