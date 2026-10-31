//! Wiki 工具层:把 [`WikiHandle`] 的能力暴露为 agent 工具。
//!
//! 与 kms-tools 同款形态:Input 结构体标 `#[tool]`(宏生成 schema),
//! 工具结构体持 `Arc<WikiHandle>`,`impl ToolFunction` 的 `run` 走
//! worker 通道。**一个工具一个文件**(`search.rs`、`edit.rs`、…),
//! 本模块只收共享 helper 与读/写两组注册函数。
//!
//! 引用解析:agent 以标题思考,所有按词条寻址的工具都收 `id` /
//! `title` 二选一([`resolve_entry`]),标题精确匹配未删除词条。

mod attachments;
mod create;
mod delete;
mod edit;
mod get;
mod links;
mod list;
mod mentions;
mod patch;
mod purge;
mod restore;
mod restore_version;
mod search;
mod update;
mod versions;

pub use attachments::{
    WikiAttachImageInput, WikiAttachImageTool, WikiAttachmentsInput, WikiAttachmentsTool,
    WikiDetachInput, WikiDetachTool, WikiGetAttachmentInput, WikiGetAttachmentTool,
};
pub use create::{WikiCreateInput, WikiCreateTool};
pub use delete::{WikiDeleteInput, WikiDeleteTool};
pub use edit::{WikiEditInput, WikiEditTool};
pub use get::{WikiGetInput, WikiGetTool};
pub use links::{WikiLinksInput, WikiLinksTool};
pub use list::{WikiListInput, WikiListTool};
pub use mentions::{WikiMentionsInput, WikiMentionsTool};
pub use patch::{WikiPatchInput, WikiPatchTool};
pub use purge::{WikiPurgeInput, WikiPurgeTool};
pub use restore::{WikiRestoreInput, WikiRestoreTool};
pub use restore_version::{WikiRestoreVersionInput, WikiRestoreVersionTool};
pub use search::{WikiSearchInput, WikiSearchTool};
pub use update::{WikiUpdateInput, WikiUpdateTool};
pub use versions::{WikiVersionsInput, WikiVersionsTool};

use std::sync::Arc;

use agentik_core::tools::{ToolError, ToolRegistration};
use serde_json::json;

use crate::error::Error;
use crate::service::WikiHandle;
use crate::storage::error::Error as StorageError;
use crate::storage::types::KnowledgeEntryData;

fn svc_err(e: impl std::fmt::Display) -> ToolError {
    ToolError::ExecutionFailed {
        source: e.to_string().into(),
    }
}

/// 存储错误 → 工具错误:agent 可自行纠正的(不存在/撞名/空标题)归
/// `ValidationFailed`(会作为错误回执返回给模型),其余归
/// `ExecutionFailed`。
fn tool_err(e: Error) -> ToolError {
    match e {
        Error::Storage(StorageError::NotFound(id)) => ToolError::ValidationFailed {
            message: format!("词条不存在(或已在回收站):{id}"),
        },
        Error::Storage(StorageError::DuplicateTitle { title, existing_id }) => {
            ToolError::ValidationFailed {
                message: format!("标题「{title}」已被词条 {existing_id} 占用"),
            }
        }
        Error::Storage(StorageError::EmptyTitle) => ToolError::ValidationFailed {
            message: "标题 trim 后为空".to_string(),
        },
        other => svc_err(other),
    }
}

/// id / title 二选一解析:都给时优先 id;都缺是输入错误。
async fn resolve_entry(
    handle: &WikiHandle,
    id: Option<String>,
    title: Option<String>,
) -> Result<KnowledgeEntryData, ToolError> {
    if let Some(id) = id {
        let uuid = parse_entry_id(&id)?;
        return handle.get_entry(uuid).await.map_err(tool_err);
    }
    let Some(title) = title else {
        return Err(ToolError::ValidationFailed {
            message: "需要提供 id 或 title 之一".to_string(),
        });
    };
    handle
        .find_by_title(&title)
        .await
        .map_err(tool_err)?
        .ok_or_else(|| ToolError::ValidationFailed {
            message: format!("没有标题为「{title}」的词条"),
        })
}

/// 把字符串 id 解析成 UUID;非法输入是 agent 可纠正错误。
fn parse_entry_id(id: &str) -> Result<uuid::Uuid, ToolError> {
    uuid::Uuid::parse_str(id).map_err(|_| ToolError::ValidationFailed {
        message: format!("非法词条 id:{id}"),
    })
}

/// 已解析词条的 id 字段转 UUID(理论必合法;失败按可纠正错误报)。
pub(super) fn invalid_entry_id(id: &str) -> ToolError {
    ToolError::ValidationFailed {
        message: format!("非法词条 id:{id}"),
    }
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// 把一批词条 id 解析成 {id, title} 列表(查不到的保留原 id)。
async fn id_title_pairs(handle: &WikiHandle, ids: Vec<String>) -> Vec<serde_json::Value> {
    handle
        .id_title_pairs(ids)
        .await
        .into_iter()
        .map(|(id, title)| json!({ "id": id, "title": title }))
        .collect()
}

/// 把新正文写回词条:mtime 推进;倒排、`[[链接]]`、版本快照由存储层
/// 同事务重建。patch/edit 失败时不会走到这里,零副作用。
async fn write_content(
    handle: &WikiHandle,
    mut entry: KnowledgeEntryData,
    new_content: String,
) -> Result<(), ToolError> {
    entry.content = new_content;
    entry.mtime = unix_now();
    handle.update_entry(entry).await.map_err(tool_err)
}

// ═══════════════════════════ 注册(读/写两组) ═══════════════════════════

/// 读工具组:任务 agent 可用,只读不动库。
pub fn wiki_read_registrations(handle: Arc<WikiHandle>) -> Vec<ToolRegistration> {
    vec![
        WikiSearchTool {
            handle: handle.clone(),
        }
        .into(),
        WikiGetTool {
            handle: handle.clone(),
        }
        .into(),
        WikiListTool {
            handle: handle.clone(),
        }
        .into(),
        WikiLinksTool {
            handle: handle.clone(),
        }
        .into(),
        WikiMentionsTool {
            handle: handle.clone(),
        }
        .into(),
        WikiAttachmentsTool {
            handle: handle.clone(),
        }
        .into(),
        WikiGetAttachmentTool {
            handle: handle.clone(),
        }
        .into(),
        WikiVersionsTool { handle }.into(),
    ]
}

/// 写工具组:知识维护 agent 专用;任务 agent 不应拿到(长期记忆保持
/// 只读)。
pub fn wiki_write_registrations(handle: Arc<WikiHandle>) -> Vec<ToolRegistration> {
    vec![
        WikiCreateTool {
            handle: handle.clone(),
        }
        .into(),
        WikiUpdateTool {
            handle: handle.clone(),
        }
        .into(),
        WikiDeleteTool {
            handle: handle.clone(),
        }
        .into(),
        WikiRestoreTool {
            handle: handle.clone(),
        }
        .into(),
        WikiPurgeTool {
            handle: handle.clone(),
        }
        .into(),
        WikiEditTool {
            handle: handle.clone(),
        }
        .into(),
        WikiPatchTool {
            handle: handle.clone(),
        }
        .into(),
        WikiAttachImageTool {
            handle: handle.clone(),
        }
        .into(),
        WikiDetachTool {
            handle: handle.clone(),
        }
        .into(),
        WikiRestoreVersionTool { handle }.into(),
    ]
}
