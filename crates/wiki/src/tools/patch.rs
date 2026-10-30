//! `wiki_patch`:Codex 格式多 chunk patch,单词条原子写。

use std::sync::Arc;

use agentik_core::tools::{ToolError, ToolFunction};
use agentik_proc::tool;
use agentik_types::tools::ToolResult;
use async_trait::async_trait;
use serde_json::json;

use super::{resolve_entry, tool_err, write_content};
use crate::service::WikiHandle;

#[tool(
    name = "wiki_patch",
    description = "Apply a Codex-format multi-hunk patch (*** Begin Patch / *** Update \
                   File / @@ context / -old +new / *** End Patch) to one wiki entry's \
                   content in a single atomic write. All hunks target the entry — file \
                   names inside the patch are ignored, Add/Delete File hunks are \
                   rejected. Fuzzy line matching, same engine as the VFS patch tool."
)]
pub struct WikiPatchInput {
    #[desc = "Entry id (UUID string)"]
    pub id: Option<String>,
    #[desc = "Exact entry title"]
    pub title: Option<String>,
    #[desc = "Patch text in Codex *** Begin Patch format; every hunk applies to the entry"]
    pub patch: String,
}

pub struct WikiPatchTool {
    pub handle: Arc<WikiHandle>,
}

#[async_trait]
impl ToolFunction for WikiPatchTool {
    type Input = WikiPatchInput;

    async fn run(&self, input: Self::Input) -> Result<ToolResult, ToolError> {
        let args =
            apply_patch::parse_patch(&input.patch).map_err(|e| ToolError::ValidationFailed {
                message: format!("patch 解析失败:{e}"),
            })?;
        if args.hunks.is_empty() {
            return Err(ToolError::ValidationFailed {
                message: "patch 不含任何 hunk".to_string(),
            });
        }
        // 单词条语义:只收 UpdateFile,全部 chunks 合并作用于正文。
        let mut chunks: Vec<apply_patch::UpdateFileChunk> = Vec::new();
        for hunk in &args.hunks {
            match hunk {
                apply_patch::Hunk::UpdateFile {
                    path, chunks: c, ..
                } => {
                    chunks.extend(c.iter().cloned());
                    let _ = path;
                }
                _ => {
                    return Err(ToolError::ValidationFailed {
                        message: "wiki_patch 只支持 *** Update File hunk(Add/Delete File \
                                  不适用于词条)"
                            .to_string(),
                    });
                }
            }
        }
        let entry = resolve_entry(&self.handle, input.id, input.title).await?;
        // 计数在 move 之前取:chunks 才是修改点的真实粒度(一个
        // UpdateFile hunk 可含多个 @@ chunk)。
        let chunks_applied = chunks.len();
        let new_content =
            apply_patch::compute_updated_content(&entry.content, &chunks).map_err(|e| {
                ToolError::ValidationFailed {
                    message: format!("patch 应用失败(未写入):{e}"),
                }
            })?;
        write_content(&self.handle, entry, new_content).await?;
        Ok(ToolResult::success_json(
            json!({ "patched": true, "chunks_applied": chunks_applied }),
        ))
    }
}
