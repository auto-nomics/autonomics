//! `wiki_mentions`:正文纯文本提及本词条但尚未链接的候选(补链来源)。

use std::sync::Arc;

use agentik_core::tools::{ToolError, ToolFunction};
use agentik_proc::tool;
use agentik_types::tools::ToolResult;
use async_trait::async_trait;
use serde_json::json;

use super::{id_title_pairs, resolve_entry, tool_err};
use crate::service::WikiHandle;

#[tool(
    name = "wiki_mentions",
    description = "Find entries whose content mentions this entry's title or alias as \
                   plain text but do NOT link to it yet — candidates for adding \
                   [[name]] links (auto-link backfill)."
)]
pub struct WikiMentionsInput {
    #[desc = "Entry id (UUID string)"]
    pub id: Option<String>,
    #[desc = "Exact entry title"]
    pub title: Option<String>,
}

pub struct WikiMentionsTool {
    pub handle: Arc<WikiHandle>,
}

#[async_trait]
impl ToolFunction for WikiMentionsTool {
    type Input = WikiMentionsInput;

    async fn run(&self, input: Self::Input) -> Result<ToolResult, ToolError> {
        let e = resolve_entry(&self.handle, input.id, input.title).await?;
        let ids = self.handle.get_mentions(&e.id).await.map_err(tool_err)?;
        let candidates = id_title_pairs(&self.handle, ids).await;
        Ok(ToolResult::success_json(json!({
            "entry": { "id": e.id, "title": e.title },
            "unlinked_mentions": candidates,
        })))
    }
}
