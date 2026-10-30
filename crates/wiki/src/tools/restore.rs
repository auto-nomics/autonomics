//! `wiki_restore`:从回收站恢复软删除的词条。

use std::sync::Arc;

use agentik_core::tools::{ToolError, ToolFunction};
use agentik_proc::tool;
use agentik_types::tools::ToolResult;
use async_trait::async_trait;
use serde_json::json;

use super::{parse_entry_id, tool_err};
use crate::service::WikiHandle;

#[tool(
    name = "wiki_restore",
    description = "Restore a soft-deleted wiki entry from the recycle bin. Its indexes \
                   and links come back intact."
)]
pub struct WikiRestoreInput {
    #[desc = "Entry id (UUID string)"]
    pub id: Option<String>,
    #[desc = "Exact entry title"]
    pub title: Option<String>,
}

pub struct WikiRestoreTool {
    pub handle: Arc<WikiHandle>,
}

#[async_trait]
impl ToolFunction for WikiRestoreTool {
    type Input = WikiRestoreInput;

    async fn run(&self, input: Self::Input) -> Result<ToolResult, ToolError> {
        // 回收站词条对 get_entry 不可见,这里只能按 id 恢复;给 title 时
        // 先查(回收站词条标题仍占用,title 查不到 → 明确报错)。
        let id = match input.id {
            Some(id) => parse_entry_id(&id)?,
            None => {
                return Err(ToolError::ValidationFailed {
                    message: "恢复需要词条 id(回收站词条无法按 title 寻址); \
                              先用 wiki_list_deleted 查 id"
                        .to_string(),
                });
            }
        };
        self.handle.restore_entry(id).await.map_err(tool_err)?;
        Ok(ToolResult::success_json(
            json!({ "restored": true, "id": id }),
        ))
    }
}
