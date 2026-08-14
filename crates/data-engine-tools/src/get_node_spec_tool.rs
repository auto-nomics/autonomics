use std::sync::Arc;

use agentik_core::tools::{ToolError, ToolFunction};
use agentik_proc::tool;
use agentik_sdk::types::ToolResult;
use async_trait::async_trait;
use data_engine::runtime::DataEngineClient;

use crate::ExecError;

#[tool(
    name = "get_node_spec",
    description = "Get the JSON Schema for a specific node kind. \
                  Returns the full parameter specification required to configure \
                  a node via add_node. Always call this before add_node to get \
                  the exact fields and types expected by the chosen node kind."
)]
pub struct GetNodeSpecInput {
    /// The node kind to query (e.g. "sql", "source_file", "sink_file", "ldsc", "linear_regression", "mock").
    pub kind: String,
}

pub struct GetNodeSpecTool {
    client: Arc<DataEngineClient>,
}

impl GetNodeSpecTool {
    pub fn new(client: Arc<DataEngineClient>) -> Self {
        Self { client }
    }
}

#[async_trait]
impl ToolFunction for GetNodeSpecTool {
    type Input = GetNodeSpecInput;

    async fn run(&self, input: Self::Input) -> Result<ToolResult, ToolError> {
        let schema = self
            .client
            .get_node_spec(&input.kind)
            .map_err(ExecError::from)?;

        let content = serde_json::to_value(&schema).map_err(|e| ToolError::ExecutionFailed {
            source: Box::new(e),
        })?;

        Ok(ToolResult::success_json(content))
    }
}
