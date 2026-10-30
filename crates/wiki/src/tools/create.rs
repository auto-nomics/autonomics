//! `wiki_create`:新建词条(`[[名称]]` 链接自动解析)。

use std::sync::Arc;

use agentik_core::tools::{ToolError, ToolFunction};
use agentik_proc::tool;
use agentik_types::tools::ToolResult;
use async_trait::async_trait;
use serde_json::json;

use super::{tool_err, unix_now};
use crate::service::WikiHandle;
use crate::storage::types::KnowledgeEntryData;

#[tool(
    name = "wiki_create",
    description = "Create a wiki entry. Use [[entry title]] in content for wiki links — \
                   links resolve by exact title or alias automatically; dangling \
                   targets are ignored. Title must be unique (trimmed)."
)]
pub struct WikiCreateInput {
    #[desc = "Unique entry title (trimmed; whitespace-only rejected)"]
    pub title: String,
    #[desc = "Markdown content — use [[entry title]] for wiki links"]
    pub content: String,
    #[desc = "Alternative names the entry is reachable by (also link-resolvable)"]
    pub alias: Option<Vec<String>>,
    #[desc = "Tags for filtered search"]
    pub tags: Option<Vec<String>>,
}

pub struct WikiCreateTool {
    pub handle: Arc<WikiHandle>,
}

#[async_trait]
impl ToolFunction for WikiCreateTool {
    type Input = WikiCreateInput;

    async fn run(&self, input: Self::Input) -> Result<ToolResult, ToolError> {
        let now = unix_now();
        let entry = KnowledgeEntryData {
            id: uuid::Uuid::new_v4().to_string(),
            title: input.title,
            alias: input.alias.unwrap_or_default(),
            tags: input.tags.unwrap_or_default(),
            content: input.content,
            ctime: now,
            mtime: now,
        };
        self.handle
            .create_entry(entry.clone())
            .await
            .map_err(tool_err)?;
        Ok(ToolResult::success_json(json!({
            "id": entry.id,
            "title": entry.title,
        })))
    }
}
