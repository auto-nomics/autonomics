use std::sync::Arc;

use agentik_core::tools::{ToolError, ToolFunction};
use agentik_proc::tool;
use agentik_sdk::types::ToolResult;
use async_trait::async_trait;
use data_engine::runtime::DataEngineClient;

use crate::ExecError;

#[tool(
    name = "update_node",
    description = "Update the spec of an existing node in the DAG. \
                  The node's kind is preserved — only the spec (configuration) \
                  changes. All existing edges are kept and re-validated. \
                  \
                  WORKFLOW — discover schema then update: \
                  1. Use `get_node_spec` with the node's kind to see the JSON \
                     Schema of valid spec fields. \
                  2. Pass the node `id` and a `spec` object conforming to the \
                     schema into this tool. \
                  \
                  Common update examples: \
                  - sql node:  {\"sql_query\": \"SELECT COUNT(*) FROM port_0\"} \
                  - file_to_dataframe: {\"path\": \"/new/data.csv\", \"format\": null} \
                  - dataframe_to_file: {\"path\": \"/out/new.csv\", \"format\": \"parquet\", \"mode\": \"overwrite\"} \
                  - linear_regression: {\"x_columns\": [\"age\"], \"y_column\": \"charges\", \"intercept\": true}"
)]
pub struct UpdateNodeInput {
    /// ID of the node to update.
    pub id: String,
    /// JSON object conforming to the node's kind JSON Schema.
    pub spec: serde_json::Value,
}

pub struct UpdateNodeTool {
    client: Arc<DataEngineClient>,
}

impl UpdateNodeTool {
    pub fn new(client: Arc<DataEngineClient>) -> Self {
        Self { client }
    }
}

#[async_trait]
impl ToolFunction for UpdateNodeTool {
    type Input = UpdateNodeInput;

    async fn run(&self, input: Self::Input) -> Result<ToolResult, ToolError> {
        let msg = format!("node '{}' updated in DAG", input.id);

        self.client
            .update_node(input.id, input.spec)
            .await
            .map_err(ExecError::from)?;

        Ok(ToolResult::success(msg))
    }
}
