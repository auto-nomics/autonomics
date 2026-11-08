//! File-system mutation operations: `mkdir`, `rm`, `cp`, `mv`.
//!
//! Thin adapters over the typed [`crate::browse`] facade so the agent
//! tool and the gateway VFS API share one implementation (including the
//! `rm` safety contract and the cross-backend copy/move fallback).

use agentik_core::tools::{ToolError, ToolResult};
use agentik_sdk::types::ToolResult as AgentToolResult;

use super::common::{require_named_path, require_path};
use crate::browse::BrowseError;
use crate::storage::OpendalFileStorage;

/// `mkdir` — create a directory.
pub async fn op_mkdir(
    storage: &OpendalFileStorage,
    path: Option<&str>,
) -> Result<AgentToolResult, ToolError> {
    let raw_path = require_path(path, "mkdir")?;
    match crate::browse::mkdir(storage, raw_path).await {
        Ok(_) => Ok(AgentToolResult::success_json(serde_json::json!({
            "path": raw_path,
            "created": true,
        }))),
        Err(error) => Ok(AgentToolResult::error(format!(
            "mkdir: {raw_path}: {}",
            error.message()
        ))),
    }
}

/// `rm` — delete a file or directory.
///
/// Safety contract:
/// - The virtual root (`/`) is always refused.
/// - Missing targets are reported as errors (no silent success).
/// - Non-empty directories are refused unless `recursive=true` is passed;
///   empty directories can be removed without the flag.
pub async fn op_rm(
    storage: &OpendalFileStorage,
    path: Option<&str>,
    recursive: Option<bool>,
) -> Result<AgentToolResult, ToolError> {
    let raw_path = require_path(path, "rm")?;
    match crate::browse::remove(storage, raw_path, recursive.unwrap_or(false)).await {
        Ok(_) => Ok(AgentToolResult::success_json(serde_json::json!({
            "path": raw_path,
            "deleted": true,
            "recursive": recursive.unwrap_or(false),
        }))),
        Err(error) => Ok(AgentToolResult::error(format!(
            "rm: {}",
            error.message()
        ))),
    }
}

/// `cp` — copy a file.
pub async fn op_cp(
    storage: &OpendalFileStorage,
    src: Option<&str>,
    dst: Option<&str>,
) -> Result<AgentToolResult, ToolError> {
    let raw_src = require_named_path(src, "src", "cp")?;
    let raw_dst = require_named_path(dst, "dst", "cp")?;
    match crate::browse::copy(storage, raw_src, raw_dst).await {
        Ok(_) => Ok(AgentToolResult::success_json(serde_json::json!({
            "src": raw_src,
            "dst": raw_dst,
            "copied": true,
        }))),
        Err(error) => Ok(AgentToolResult::error(format!(
            "cp: {}",
            error.message()
        ))),
    }
}

/// `mv` — rename/move a file.
pub async fn op_mv(
    storage: &OpendalFileStorage,
    src: Option<&str>,
    dst: Option<&str>,
) -> Result<AgentToolResult, ToolError> {
    let raw_src = require_named_path(src, "src", "mv")?;
    let raw_dst = require_named_path(dst, "dst", "mv")?;
    match crate::browse::move_entry(storage, raw_src, raw_dst).await {
        Ok(_) => Ok(AgentToolResult::success_json(serde_json::json!({
            "src": raw_src,
            "dst": raw_dst,
            "moved": true,
        }))),
        Err(error) => Ok(AgentToolResult::error(format!(
            "mv: {}",
            error.message()
        ))),
    }
}
