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
                  Call this before add_edge to know which ports are available. \
                  For dynamic-port kinds (notably container_command), pass the exact \
                  `spec` you plan to add so the returned output ports match the \
                  spec's declared outputs."
)]
pub struct GetNodePortsInput {
    /// The node kind to query (e.g. "sql", "file_to_dataframe", "dataframe_to_file", "ldsc", "linear_regression", "mock").
    pub kind: String,
    /// Optional concrete node spec. Provide this for dynamic-port kinds.
    #[serde(default)]
    pub spec: Option<serde_json::Value>,
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
        let ports = match input.spec {
            Some(spec) => self
                .client
                .get_node_ports_for_spec(&input.kind, spec)
                .map_err(ExecError::from)?,
            None => self
                .client
                .get_node_ports(&input.kind)
                .map_err(ExecError::from)?,
        };

        let content = serde_json::to_value(&ports).map_err(|e| ToolError::ExecutionFailed {
            source: Box::new(e),
        })?;

        Ok(ToolResult::success_json(content))
    }
}
