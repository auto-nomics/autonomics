use std::sync::Arc;

use agentik_core::tools::{ToolError, ToolFunction};
use agentik_proc::tool;
use agentik_sdk::types::ToolResult;
use async_trait::async_trait;
use data_engine::runtime::DataEngineClient;

use crate::ExecError;

#[tool(
    name = "get_node_ports",
    description = "Get the input/output port layout of a specific node kind. \
                  Returns declared ports needed to wire edges via add_edge. \
                  Call this before add_edge to know which ports are available."
)]
pub struct GetNodePortsInput {
    /// The node kind to query (e.g. "sql", "source_file", "sink_file", "ldsc", "linear_regression", "mock").
    pub kind: String,
}

pub struct GetNodePortsTool {
    client: Arc<DataEngineClient>,
}

impl GetNodePortsTool {
    pub fn new(client: Arc<DataEngineClient>) -> Self {
        Self { client }
    }
}

#[async_trait]
impl ToolFunction for GetNodePortsTool {
    type Input = GetNodePortsInput;

    async fn run(&self, input: Self::Input) -> Result<ToolResult, ToolError> {
        let ports = self
            .client
            .get_node_ports(&input.kind)
            .map_err(ExecError::from)?;

        let content = serde_json::to_value(&ports).map_err(|e| ToolError::ExecutionFailed {
            source: Box::new(e),
        })?;

        Ok(ToolResult::success_json(content))
    }
}
