//! `wiki_versions`:版本快照历史(新→旧,仅元信息)。

use std::sync::Arc;

use agentik_core::tools::{ToolError, ToolFunction};
use agentik_proc::tool;
use agentik_types::tools::ToolResult;
use async_trait::async_trait;
use serde_json::json;

use super::{parse_entry_id, resolve_entry, tool_err};
use crate::service::WikiHandle;

#[tool(
    name = "wiki_versions",
    description = "List an entry's version snapshots (newest first). Bodies are not \
                   included; roll back with wiki_restore_version."
)]
pub struct WikiVersionsInput {
    #[desc = "Entry id (UUID string)"]
    pub id: Option<String>,
    #[desc = "Exact entry title"]
    pub title: Option<String>,
}

pub struct WikiVersionsTool {
    pub handle: Arc<WikiHandle>,
}

#[async_trait]
impl ToolFunction for WikiVersionsTool {
    type Input = WikiVersionsInput;

    async fn run(&self, input: Self::Input) -> Result<ToolResult, ToolError> {
        let e = resolve_entry(&self.handle, input.id, input.title).await?;
        let id = parse_entry_id(&e.id)?;
        let versions = self.handle.list_versions(id).await.map_err(tool_err)?;
        Ok(ToolResult::success_json(json!({
            "entry": { "id": e.id, "title": e.title },
            "versions": versions.iter().map(|v| json!({
                "version": v.version,
                "created_at": v.created_at,
            })).collect::<Vec<_>>(),
        })))
    }
}
