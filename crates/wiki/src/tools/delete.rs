//! `wiki_delete`:软删除进回收站(可恢复)。

use std::sync::Arc;

use agentik_core::tools::{ToolError, ToolFunction};
use agentik_proc::tool;
use agentik_types::tools::ToolResult;
use async_trait::async_trait;
use serde_json::json;

use super::{parse_entry_id, resolve_entry, tool_err};
use crate::service::WikiHandle;

#[tool(
    name = "wiki_delete",
    description = "Soft-delete a wiki entry to the recycle bin: it disappears from \
                   reads/search/links but keeps occupying its title until \
                   wiki_purge (permanent) or wiki_restore. Prefer this over purge."
)]
pub struct WikiDeleteInput {
    #[desc = "Entry id (UUID string)"]
    pub id: Option<String>,
    #[desc = "Exact entry title"]
    pub title: Option<String>,
}

pub struct WikiDeleteTool {
    pub handle: Arc<WikiHandle>,
}

#[async_trait]
impl ToolFunction for WikiDeleteTool {
    type Input = WikiDeleteInput;

    async fn run(&self, input: Self::Input) -> Result<ToolResult, ToolError> {
        let e = resolve_entry(&self.handle, input.id, input.title).await?;
        let id = parse_entry_id(&e.id)?;
        self.handle.delete_entry(id).await.map_err(tool_err)?;
        Ok(ToolResult::success_json(json!({
            "deleted": true,
            "id": e.id,
            "title": e.title,
            "recoverable": "wiki_restore",
        })))
    }
}
