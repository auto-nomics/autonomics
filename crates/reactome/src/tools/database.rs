use std::sync::Arc;

use agentik_core::tools::{ToolError, ToolFunction};
use agentik_proc::tool;
use agentik_sdk::types::ToolResult as AgentToolResult;
use async_trait::async_trait;

use super::json_err;
use crate::ReactomeClient;
use crate::format::format_database;

#[tool(
    name = "reactome_database",
    description = "Retrieve the Reactome database name and release version."
)]
pub struct DatabaseInput {}

pub struct ReactomeDatabaseTool {
    pub(crate) client: Arc<ReactomeClient>,
}

#[async_trait]
impl ToolFunction for ReactomeDatabaseTool {
    type Input = DatabaseInput;

    async fn run(&self, _input: Self::Input) -> Result<AgentToolResult, ToolError> {
        let info = self.client.database_info().await.map_err(json_err)?;
        Ok(AgentToolResult::success(format_database(&info)))
    }
}
