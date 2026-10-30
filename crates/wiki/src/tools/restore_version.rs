//! `wiki_restore_version`:回滚到历史版本(回滚本身追加新版本)。

use std::sync::Arc;

use agentik_core::tools::{ToolError, ToolFunction};
use agentik_proc::tool;
use agentik_types::tools::ToolResult;
use async_trait::async_trait;
use serde_json::json;

use super::{parse_entry_id, resolve_entry, tool_err};
use crate::service::WikiHandle;

#[tool(
    name = "wiki_restore_version",
    description = "Roll an entry back to a historical version (see wiki_versions). \
                   The rollback itself appends a new version — history is never lost."
)]
pub struct WikiRestoreVersionInput {
    #[desc = "Entry id (UUID string)"]
    pub id: Option<String>,
    #[desc = "Exact entry title"]
    pub title: Option<String>,
    #[desc = "Version number to roll back to"]
    pub version: i64,
}

pub struct WikiRestoreVersionTool {
    pub handle: Arc<WikiHandle>,
}

#[async_trait]
impl ToolFunction for WikiRestoreVersionTool {
    type Input = WikiRestoreVersionInput;

    async fn run(&self, input: Self::Input) -> Result<ToolResult, ToolError> {
        let e = resolve_entry(&self.handle, input.id, input.title).await?;
        let id = parse_entry_id(&e.id)?;
        self.handle
            .restore_version(id, input.version)
            .await
            .map_err(tool_err)?;
        let current = self.handle.get_entry(id).await.map_err(tool_err)?;
        Ok(ToolResult::success_json(json!({
            "id": current.id,
            "title": current.title,
            "rolled_back_to": input.version,
        })))
    }
}
