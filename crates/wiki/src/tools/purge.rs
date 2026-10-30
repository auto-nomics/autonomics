//! `wiki_purge`:净删(不可逆,释放标题,清空全部衍生数据)。

use std::sync::Arc;

use agentik_core::tools::{ToolError, ToolFunction};
use agentik_proc::tool;
use agentik_types::tools::ToolResult;
use async_trait::async_trait;
use serde_json::json;

use super::{parse_entry_id, resolve_entry, tool_err};
use crate::service::WikiHandle;

#[tool(
    name = "wiki_purge",
    description = "PERMANENTLY delete a wiki entry: version history, links, and search \
                   index are scrubbed and the title is freed. Irreversible — \
                   prefer wiki_delete unless the caller explicitly asked for purge."
)]
pub struct WikiPurgeInput {
    #[desc = "Entry id (UUID string)"]
    pub id: Option<String>,
    #[desc = "Exact entry title"]
    pub title: Option<String>,
}

pub struct WikiPurgeTool {
    pub handle: Arc<WikiHandle>,
}

#[async_trait]
impl ToolFunction for WikiPurgeTool {
    type Input = WikiPurgeInput;

    async fn run(&self, input: Self::Input) -> Result<ToolResult, ToolError> {
        // 净删允许作用于回收站词条:先试可见解析,失败再查回收站清单
        // 按 title 匹配。
        let id: uuid::Uuid =
            match resolve_entry(&self.handle, input.id.clone(), input.title.clone()).await {
                Ok(e) => parse_entry_id(&e.id)?,
                Err(_) => {
                    let title =
                        input
                            .title
                            .as_deref()
                            .ok_or_else(|| ToolError::ValidationFailed {
                                message: "词条不可见且未提供 title".to_string(),
                            })?;
                    let bin = self.handle.list_deleted().await.map_err(tool_err)?;
                    bin.iter()
                        .find(|item| item.entry.title == title)
                        .and_then(|item| uuid::Uuid::parse_str(&item.entry.id).ok())
                        .ok_or_else(|| ToolError::ValidationFailed {
                            message: format!("没有标题为「{title}」的词条(含回收站)"),
                        })?
                }
            };
        self.handle.purge_entry(id).await.map_err(tool_err)?;
        Ok(ToolResult::success_json(
            json!({ "purged": true, "id": id }),
        ))
    }
}
