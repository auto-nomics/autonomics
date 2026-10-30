//! `wiki_update`:元数据更新(改名/别名/标签),每次写快照版本。
//!
//! 不收 `content`:正文改动一律走 `wiki_edit`(单处替换)或
//! `wiki_patch`(多 hunk),它们按构造保留锚点外内容,且失败即拒写;
//! 整篇替换的口子留在存储层 `update_entry`,不暴露给工具面。

use std::sync::Arc;

use agentik_core::tools::{ToolError, ToolFunction};
use agentik_proc::tool;
use agentik_types::tools::ToolResult;
use async_trait::async_trait;
use serde_json::json;

use super::{resolve_entry, tool_err, unix_now};
use crate::service::WikiHandle;

#[tool(
    name = "wiki_update",
    description = "Update a wiki entry's metadata (merge semantics: only provided fields \
                   change). `new_title` renames (must stay unique); alias/tags replace \
                   their full lists. Content is NOT changed here — use wiki_edit for a \
                   single replacement or wiki_patch for multiple hunks. Every update \
                   snapshots a new version; roll back with wiki_restore_version."
)]
pub struct WikiUpdateInput {
    #[desc = "Entry id (UUID string)"]
    pub id: Option<String>,
    #[desc = "Exact current title (lookup key)"]
    pub title: Option<String>,
    #[desc = "Rename the entry to this title"]
    pub new_title: Option<String>,
    #[desc = "Replacement alias list (replaces all)"]
    pub alias: Option<Vec<String>>,
    #[desc = "Replacement tag list (replaces all)"]
    pub tags: Option<Vec<String>>,
}

pub struct WikiUpdateTool {
    pub handle: Arc<WikiHandle>,
}

#[async_trait]
impl ToolFunction for WikiUpdateTool {
    type Input = WikiUpdateInput;

    async fn run(&self, input: Self::Input) -> Result<ToolResult, ToolError> {
        let mut entry = resolve_entry(&self.handle, input.id, input.title).await?;
        if let Some(new_title) = input.new_title {
            entry.title = new_title;
        }
        if let Some(alias) = input.alias {
            entry.alias = alias;
        }
        if let Some(tags) = input.tags {
            entry.tags = tags;
        }
        entry.mtime = unix_now();
        self.handle
            .update_entry(entry.clone())
            .await
            .map_err(tool_err)?;
        Ok(ToolResult::success_json(json!({
            "id": entry.id,
            "title": entry.title,
            "tags": entry.tags,
        })))
    }
}
