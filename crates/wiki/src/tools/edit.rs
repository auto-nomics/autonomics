//! `wiki_edit`:fuzzy str_replace 增量编辑正文(与 VFS edit 同引擎)。

use std::sync::Arc;

use agentik_core::tools::{ToolError, ToolFunction};
use agentik_proc::tool;
use agentik_types::tools::ToolResult;
use async_trait::async_trait;
use serde_json::json;

use super::{resolve_entry, tool_err, write_content};
use crate::service::WikiHandle;

#[tool(
    name = "wiki_edit",
    description = "Incremental edit of a wiki entry's content: replace an exact old_string \
                   with new_string (fuzzy line matching: exact / rstrip / trim / \
                   Unicode-normalised, same engine as the VFS edit tool). Fails without \
                   writing when the pattern is not found or is ambiguous."
)]
pub struct WikiEditInput {
    #[desc = "Entry id (UUID string)"]
    pub id: Option<String>,
    #[desc = "Exact entry title"]
    pub title: Option<String>,
    #[desc = "Text to replace (matched as whole lines with fuzzy tolerance)"]
    pub old_string: String,
    #[desc = "Replacement text"]
    pub new_string: String,
    #[desc = "Replace every occurrence instead of requiring exactly one (default false)"]
    pub replace_all: Option<bool>,
}

pub struct WikiEditTool {
    pub handle: Arc<WikiHandle>,
}

#[async_trait]
impl ToolFunction for WikiEditTool {
    type Input = WikiEditInput;

    async fn run(&self, input: Self::Input) -> Result<ToolResult, ToolError> {
        if input.old_string == input.new_string {
            return Err(ToolError::ValidationFailed {
                message: "old_string 与 new_string 不能相同".to_string(),
            });
        }
        let entry = resolve_entry(&self.handle, input.id, input.title).await?;
        let replace_all = input.replace_all.unwrap_or(false);
        let (new_content, count, fuzzy) = match apply_patch::fuzzy_edit(
            &entry.content,
            &input.old_string,
            &input.new_string,
            replace_all,
        ) {
            apply_patch::FuzzyEditOutcome::Replaced {
                new_content,
                count,
                fuzzy,
            } => (new_content, count, fuzzy),
            apply_patch::FuzzyEditOutcome::NotFound => {
                return Err(ToolError::ValidationFailed {
                    message: "old_string 未命中(已尝试精确 / 去尾空白 / trim / \
                              Unicode 归一四种匹配);先 wiki_get 核对原文"
                        .to_string(),
                });
            }
            apply_patch::FuzzyEditOutcome::Ambiguous { count } => {
                return Err(ToolError::ValidationFailed {
                    message: format!(
                        "old_string 命中 {count} 处;设置 replace_all=true 或加长上下文使其唯一"
                    ),
                });
            }
        };
        write_content(&self.handle, entry, new_content).await?;
        Ok(ToolResult::success_json(json!({
            "edited": true,
            "replacements": count,
            "fuzzy": fuzzy,
        })))
    }
}
