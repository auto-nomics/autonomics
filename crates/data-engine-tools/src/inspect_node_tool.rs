use std::sync::Arc;

use agentik_core::tools::{ToolError, ToolFunction};
use agentik_proc::tool;
use agentik_sdk::types::ToolResult;
use async_trait::async_trait;
use data_engine::runtime::DataEngineClient;

use crate::ExecError;

#[tool(
    name = "inspect_node",
    description = "Get the current configuration of an existing node instance in \
                  the DAG. Returns the node's `kind` and stored `spec` (the JSON \
                  configuration installed or updated through dag_shell). \
                  \
                  Use this to inspect what is actually configured on a node before \
                  updating it, debugging why a dag_shell operation was \
                  rejected, or confirming the live spec matches your intent. \
                  \
                  Distinct from `get_node_spec`, which returns the parameter \
                  *schema* (template) for a node kind — `inspect_node` returns the \
                  concrete configuration currently stored on a node instance."
)]
pub struct InspectNodeInput {
    /// The node id to inspect (the id used in dag_shell, not the kind).
    pub id: String,
}

pub struct InspectNodeTool {
    client: Arc<DataEngineClient>,
}

impl InspectNodeTool {
    pub fn new(client: Arc<DataEngineClient>) -> Self {
        Self { client }
    }
}

#[async_trait]
impl ToolFunction for InspectNodeTool {
    type Input = InspectNodeInput;

    async fn run(&self, input: Self::Input) -> Result<ToolResult, ToolError> {
        let Some((kind, spec)) = self
            .client
            .get_node(input.id.clone())
            .await
            .map_err(ExecError::from)?
        else {
            // No retained spec for this id. Distinguish "no such node" from
            // "node exists but has no spec" so the caller gets an actionable
            // hint rather than a bare null.
            let exists = self
                .client
                .node_exists(input.id.clone())
                .await
                .map_err(ExecError::from)?;

            let hint = if exists {
                format!(
                    "Node '{}' exists but has no retained spec (it was added via \
                     a raw path that bypasses the registry). Its configuration \
                     cannot be inspected; remove and re-add it via `dag_shell` if \
                     you need an inspectable spec.",
                    input.id
                )
            } else {
                format!(
                    "Node '{}' does not exist in the current DAG. Use `view_dag` \
                     to list node ids, then retry `inspect_node`.",
                    input.id
                )
            };

            return Ok(ToolResult::error(hint));
        };

        let content = serde_json::json!({
            "id": input.id,
            "kind": kind,
            "spec": spec,
        });

        Ok(ToolResult::success_json(content))
    }
}
