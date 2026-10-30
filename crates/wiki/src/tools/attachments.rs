//! 附件工具:图像的附加/清单/取回/摘除(blob 走 BlobStore 后端)。

use std::sync::Arc;

use agentik_core::tools::{ToolError, ToolFunction};
use agentik_proc::tool;
use agentik_types::tools::ToolResult;
use async_trait::async_trait;
use base64::Engine as _;
use serde_json::json;

use super::{resolve_entry, tool_err};
use crate::service::WikiHandle;

/// 工具层统一的 base64 编解码(STD 引擎,带 padding)。
fn decode_base64(input: &str) -> Result<Vec<u8>, ToolError> {
    base64::engine::general_purpose::STANDARD
        .decode(input.trim())
        .map_err(|e| ToolError::ValidationFailed {
            message: format!("data_base64 不是合法 base64:{e}"),
        })
}

/// 附件引用的 markdown 片段(展示层解析 `wiki-attach://` 的约定 URI)。
fn markdown_ref(meta: &crate::storage::types::AttachmentMeta) -> String {
    format!(
        "![{}]({}://{}/{})",
        meta.filename, "wiki-attach", meta.id, meta.filename
    )
}

#[tool(
    name = "wiki_attach_image",
    description = "Attach an image (base64) to a wiki entry. The filename extension picks \
                   the MIME (png/jpg/jpeg/gif/webp/svg/bmp/avif only); content is stored \
                   in the blob backend and metadata links it to the entry. Returns the \
                   attachment id and a ready-to-paste markdown image reference."
)]
pub struct WikiAttachImageInput {
    #[desc = "Entry id (UUID string)"]
    pub id: Option<String>,
    #[desc = "Exact entry title"]
    pub title: Option<String>,
    #[desc = "Original file name; its extension determines the MIME type"]
    pub filename: String,
    #[desc = "Image bytes encoded as standard base64"]
    pub data_base64: String,
}

pub struct WikiAttachImageTool {
    pub handle: Arc<WikiHandle>,
}

#[async_trait]
impl ToolFunction for WikiAttachImageTool {
    type Input = WikiAttachImageInput;

    async fn run(&self, input: Self::Input) -> Result<ToolResult, ToolError> {
        let entry = resolve_entry(&self.handle, input.id, input.title).await?;
        let entry_id =
            uuid::Uuid::parse_str(&entry.id).map_err(|_| super::invalid_entry_id(&entry.id))?;
        let data = decode_base64(&input.data_base64)?;
        let meta = self
            .handle
            .attach_image(entry_id, input.filename, data)
            .await
            .map_err(tool_err)?;
        Ok(ToolResult::success_json(json!({
            "attachment_id": meta.id,
            "filename": meta.filename,
            "mime": meta.mime,
            "size": meta.size,
            "sha256": meta.sha256,
            "markdown": markdown_ref(&meta),
        })))
    }
}

#[tool(
    name = "wiki_attachments",
    description = "List a wiki entry's image attachments (metadata only: filename, mime, \
                   size, sha256, id). Fetch bytes with wiki_get_attachment."
)]
pub struct WikiAttachmentsInput {
    #[desc = "Entry id (UUID string)"]
    pub id: Option<String>,
    #[desc = "Exact entry title"]
    pub title: Option<String>,
}

pub struct WikiAttachmentsTool {
    pub handle: Arc<WikiHandle>,
}

#[async_trait]
impl ToolFunction for WikiAttachmentsTool {
    type Input = WikiAttachmentsInput;

    async fn run(&self, input: Self::Input) -> Result<ToolResult, ToolError> {
        let entry = resolve_entry(&self.handle, input.id, input.title).await?;
        let entry_id =
            uuid::Uuid::parse_str(&entry.id).map_err(|_| super::invalid_entry_id(&entry.id))?;
        let items = self
            .handle
            .list_attachments(entry_id)
            .await
            .map_err(tool_err)?;
        let count = items.len();
        Ok(ToolResult::success_json(json!({
            "entry": { "id": entry.id, "title": entry.title },
            "count": count,
            "attachments": items.iter().map(|m| json!({
                "attachment_id": m.id,
                "filename": m.filename,
                "mime": m.mime,
                "size": m.size,
                "sha256": m.sha256,
                "created_at": m.created_at,
            })).collect::<Vec<_>>(),
        })))
    }
}

#[tool(
    name = "wiki_get_attachment",
    description = "Fetch one attachment's bytes (base64) and metadata by attachment id."
)]
pub struct WikiGetAttachmentInput {
    #[desc = "Entry id (UUID string)"]
    pub id: Option<String>,
    #[desc = "Exact entry title"]
    pub title: Option<String>,
    #[desc = "Attachment id (UUID string, from wiki_attachments)"]
    pub attachment_id: String,
}

pub struct WikiGetAttachmentTool {
    pub handle: Arc<WikiHandle>,
}

#[async_trait]
impl ToolFunction for WikiGetAttachmentTool {
    type Input = WikiGetAttachmentInput;

    async fn run(&self, input: Self::Input) -> Result<ToolResult, ToolError> {
        let entry = resolve_entry(&self.handle, input.id, input.title).await?;
        let entry_id =
            uuid::Uuid::parse_str(&entry.id).map_err(|_| super::invalid_entry_id(&entry.id))?;
        let attachment_id = uuid::Uuid::parse_str(&input.attachment_id).map_err(|_| {
            ToolError::ValidationFailed {
                message: format!("非法附件 id:{}", input.attachment_id),
            }
        })?;
        let (meta, data) = self
            .handle
            .get_attachment(entry_id, attachment_id)
            .await
            .map_err(tool_err)?;
        Ok(ToolResult::success_json(json!({
            "attachment_id": meta.id,
            "filename": meta.filename,
            "mime": meta.mime,
            "size": meta.size,
            "sha256": meta.sha256,
            "data_base64": base64::engine::general_purpose::STANDARD.encode(data),
        })))
    }
}

#[tool(
    name = "wiki_detach",
    description = "Remove an attachment from a wiki entry: metadata row is deleted and \
                   the blob is dropped. Idempotent."
)]
pub struct WikiDetachInput {
    #[desc = "Entry id (UUID string)"]
    pub id: Option<String>,
    #[desc = "Exact entry title"]
    pub title: Option<String>,
    #[desc = "Attachment id (UUID string, from wiki_attachments)"]
    pub attachment_id: String,
}

pub struct WikiDetachTool {
    pub handle: Arc<WikiHandle>,
}

#[async_trait]
impl ToolFunction for WikiDetachTool {
    type Input = WikiDetachInput;

    async fn run(&self, input: Self::Input) -> Result<ToolResult, ToolError> {
        let entry = resolve_entry(&self.handle, input.id, input.title).await?;
        let entry_id =
            uuid::Uuid::parse_str(&entry.id).map_err(|_| super::invalid_entry_id(&entry.id))?;
        let attachment_id = uuid::Uuid::parse_str(&input.attachment_id).map_err(|_| {
            ToolError::ValidationFailed {
                message: format!("非法附件 id:{}", input.attachment_id),
            }
        })?;
        self.handle
            .detach_attachment(entry_id, attachment_id)
            .await
            .map_err(tool_err)?;
        Ok(ToolResult::success_json(
            json!({ "detached": true, "attachment_id": attachment_id }),
        ))
    }
}
