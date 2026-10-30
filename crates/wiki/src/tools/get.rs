//! `wiki_get`:按 id 或精确标题取单个词条全文。

use std::sync::Arc;

use agentik_core::tools::{ToolError, ToolFunction};
use agentik_proc::tool;
use agentik_types::tools::ToolResult;
use async_trait::async_trait;
use serde_json::json;

use super::resolve_entry;
use crate::service::WikiHandle;

#[tool(
    name = "wiki_get",
    description = "Fetch one wiki entry by id or exact title (alias not accepted here). \
                   Returns full content, alias, tags, and timestamps."
)]
pub struct WikiGetInput {
    #[desc = "Entry id (UUID string)"]
    pub id: Option<String>,
    #[desc = "Exact entry title"]
    pub title: Option<String>,
}

pub struct WikiGetTool {
    pub handle: Arc<WikiHandle>,
}

#[async_trait]
impl ToolFunction for WikiGetTool {
    type Input = WikiGetInput;

    async fn run(&self, input: Self::Input) -> Result<ToolResult, ToolError> {
        let e = resolve_entry(&self.handle, input.id, input.title).await?;
        Ok(ToolResult::success_json(json!({
            "id": e.id,
            "title": e.title,
            "alias": e.alias,
            "tags": e.tags,
            "content": e.content,
            "ctime": e.ctime,
            "mtime": e.mtime,
        })))
    }
}
