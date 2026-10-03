use std::sync::Arc;

use agentik_core::tools::{ToolError, ToolFunction};
use agentik_proc::tool;
use agentik_sdk::types::ToolResult;
use async_trait::async_trait;
use data_engine::runtime::DataEngineClient;

use crate::ExecError;

#[tool(
    name = "get_node_doc",
    description = "Get the documentation string for a specific node kind. \
                  Returns a human-readable description of the node's purpose, \
                  semantics and usage hints. Call this when you need more detail \
                  than the short description in list_node_factories."
)]
pub struct GetNodeDocInput {
    /// The node kind to query (e.g. "sql", "file_to_dataframe", "dataframe_to_file", "sldsc", "linear_regression", "echo").
    pub kind: String,
}

pub struct GetNodeDocTool {
    client: Arc<DataEngineClient>,
}

impl GetNodeDocTool {
    pub fn new(client: Arc<DataEngineClient>) -> Self {
        Self { client }
    }
}

#[async_trait]
impl ToolFunction for GetNodeDocTool {
    type Input = GetNodeDocInput;

    async fn run(&self, input: Self::Input) -> Result<ToolResult, ToolError> {
        let doc = self
            .client
            .get_node_doc(&input.kind)
            .map_err(ExecError::from)?;

        Ok(ToolResult::success_json(serde_json::json!({ "doc": doc })))
    }
}
