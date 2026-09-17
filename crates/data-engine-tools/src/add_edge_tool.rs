use std::sync::Arc;

use agentik_sdk::types::ToolResult;
use async_trait::async_trait;
use data_engine::runtime::DataEngineClient;

use agentik_core::tools::{ToolError, ToolFunction};
use agentik_proc::tool;

use crate::ExecError;

#[tool(
    name = "add_edge",
    description = "Connect two DAG nodes port-to-port: data flows from the \
                  'from' node's output port to the 'to' node's input port. \
                  All four arguments are required — there is no default-port \
                  fallback. Use `get_node_ports` to discover the correct \
                  output/input port indices and semantic labels before calling. \
                  When both ports declare file formats, incompatible formats \
                  are rejected here instead of failing during execution. \
                  Each input port accepts at most one incoming edge; for a \
                  variadic node such as `sql`, connect additional upstreams \
                  to distinct target ports (0, 1, 2, ...). \
                  \
                  The tool briefly waits if a node creation request from the \
                  same response turn is still in flight. Creating nodes first \
                  and wiring edges afterward remains the preferred workflow."
)]
pub struct AddEdgeInput {
    #[desc = "ID of the upstream (source) node"]
    pub from: String,
    #[desc = "Output port index on the 'from' node (use `get_node_ports` to look it up)"]
    pub from_port: u8,
    #[desc = "ID of the downstream (target) node"]
    pub to: String,
    #[desc = "Input port index on the 'to' node (use `get_node_ports` to look it up)"]
    pub to_port: u8,
}

pub struct AddEdgeTool {
    client: Arc<DataEngineClient>,
}

impl AddEdgeTool {
    pub fn new(client: Arc<DataEngineClient>) -> Self {
        Self { client }
    }
}

#[async_trait]
impl ToolFunction for AddEdgeTool {
    type Input = AddEdgeInput;

    async fn run(&self, input: Self::Input) -> Result<ToolResult, ToolError> {
        // Tool calls in one MCP response can be dispatched concurrently. The
        // actor commits each node synchronously, so waiting for both IDs to be
        // visible before sending AddEdge also restores FIFO ordering.
        for _ in 0..20 {
            let from_exists = self
                .client
                .node_exists(input.from.clone())
                .await
                .map_err(ExecError::from)?;
            let to_exists = self
                .client
                .node_exists(input.to.clone())
                .await
                .map_err(ExecError::from)?;
            if from_exists && to_exists {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }

        let msg = format!(
            "edge added: {}.{} -> {}.{}",
            input.from, input.from_port, input.to, input.to_port
        );

        self.client
            .add_edge_port(input.from, input.from_port, input.to, input.to_port)
            .await
            .map_err(ExecError::from)?;

        Ok(ToolResult::success(msg))
    }
}
