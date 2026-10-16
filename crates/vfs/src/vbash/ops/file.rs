//! File-system mutation operations: `mkdir`, `rm`, `cp`, `mv`.

use agentik_core::tools::{ToolError, ToolResult};
use agentik_sdk::types::ToolResult as AgentToolResult;

use super::common::{require_named_path, require_path};
use crate::storage::OpendalFileStorage;

/// `mkdir` — create a directory.
pub async fn op_mkdir(
    storage: &OpendalFileStorage,
    path: Option<&str>,
) -> Result<AgentToolResult, ToolError> {
    let raw_path = require_path(path, "mkdir")?;
    let vpath = OpendalFileStorage::normalize_path(raw_path);
    if let Err(e) = storage.check_writable(&vpath) {
        return Ok(AgentToolResult::error(format!("mkdir: {raw_path}: {e}")));
    }
    let op = storage.resolve(&vpath);
    let remote = storage.resolve_path(&vpath);

    // OpenDAL Fs backend requires a trailing '/' for directory creation.
    let dir_path = if remote.is_empty() {
        "/".to_string()
    } else if remote.ends_with('/') {
        remote
    } else {
        format!("{remote}/")
    };

    op.create_dir(&dir_path).await.map_err(|e| e.to_string())?;

    Ok(AgentToolResult::success_json(serde_json::json!({
        "path": raw_path,
        "created": true,
    })))
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
    let vpath = OpendalFileStorage::normalize_path(raw_path);

    if vpath == "/" {
        return Ok(AgentToolResult::error(
            "Refusing to rm '/': that is the virtual filesystem root. \
             Use a sub-path like '/tmp' instead.",
        ));
    }
    if let Err(e) = storage.check_writable(&vpath) {
        return Ok(AgentToolResult::error(format!("rm: {raw_path}: {e}")));
    }
    let op = storage.resolve(&vpath);
    let remote = storage.resolve_path(&vpath);

    let meta = match op.stat(&remote).await {
        Ok(m) => m,
        Err(e) if matches!(e.kind(), opendal::ErrorKind::NotFound) => {
            return Ok(AgentToolResult::error(format!(
                "rm: cannot remove '{raw_path}': No such file or directory"
            )));
        }
        Err(e) => return Ok(AgentToolResult::error(format!("rm: {e}"))),
    };

    let recursive_flag = recursive.unwrap_or(false);

    if meta.is_dir() && !recursive_flag {
        // Empty-directory removal: succeeds only if the directory has no
        // entries. Non-empty directories are explicitly refused here so
        // the caller sees a clear message instead of a silent wipe.
        return match op.delete(&remote).await {
            Ok(()) => Ok(AgentToolResult::success_json(serde_json::json!({
                "path": raw_path,
                "deleted": true,
                "recursive": false,
            }))),
            Err(e) => Ok(AgentToolResult::error(format!(
                "rm: cannot remove '{raw_path}': {e}. \
                 If the directory is non-empty, pass recursive=true to remove it and its contents."
            ))),
        };
    }

    op.delete_with(&remote)
        .recursive(recursive_flag)
        .await
        .map_err(|e| e.to_string())?;

    Ok(AgentToolResult::success_json(serde_json::json!({
        "path": raw_path,
        "deleted": true,
        "recursive": recursive_flag,
    })))
}

/// `cp` — copy a file.
pub async fn op_cp(
    storage: &OpendalFileStorage,
    src: Option<&str>,
    dst: Option<&str>,
) -> Result<AgentToolResult, ToolError> {
    let raw_src = require_named_path(src, "src", "cp")?;
    let raw_dst = require_named_path(dst, "dst", "cp")?;
    let vsrc = OpendalFileStorage::normalize_path(raw_src);
    let vdst = OpendalFileStorage::normalize_path(raw_dst);
    if let Err(e) = storage.check_readable(&vsrc) {
        return Ok(AgentToolResult::error(format!("cp: {raw_src}: {e}")));
    }
    if let Err(e) = storage.check_writable(&vdst) {
        return Ok(AgentToolResult::error(format!("cp: {raw_dst}: {e}")));
    }
    let src_op = storage.resolve(&vsrc);
    let dst_op = storage.resolve(&vdst);
    let src_remote = storage.resolve_path(&vsrc);
    let dst_remote = storage.resolve_path(&vdst);
    // OpenDAL copy is backend-internal. If src and dst resolve to
    // different backends, fall back to read+write.
    if !storage.same_backend(&vsrc, &vdst) {
        let buf = src_op.read(&src_remote).await.map_err(|e| e.to_string())?;
        dst_op
            .write(&dst_remote, buf)
            .await
            .map_err(|e| e.to_string())?;
    } else {
        src_op
            .copy(&src_remote, &dst_remote)
            .await
            .map_err(|e| e.to_string())?;
    }

    Ok(AgentToolResult::success_json(serde_json::json!({
        "src": raw_src,
        "dst": raw_dst,
        "copied": true,
    })))
}

/// `mv` — rename/move a file.
pub async fn op_mv(
    storage: &OpendalFileStorage,
    src: Option<&str>,
    dst: Option<&str>,
) -> Result<AgentToolResult, ToolError> {
    let raw_src = require_named_path(src, "src", "mv")?;
    let raw_dst = require_named_path(dst, "dst", "mv")?;
    let vsrc = OpendalFileStorage::normalize_path(raw_src);
    let vdst = OpendalFileStorage::normalize_path(raw_dst);
    if let Err(e) = storage.check_writable(&vsrc) {
        return Ok(AgentToolResult::error(format!("mv: {raw_src}: {e}")));
    }
    if let Err(e) = storage.check_writable(&vdst) {
        return Ok(AgentToolResult::error(format!("mv: {raw_dst}: {e}")));
    }
    let src_op = storage.resolve(&vsrc);
    let dst_op = storage.resolve(&vdst);
    let src_remote = storage.resolve_path(&vsrc);
    let dst_remote = storage.resolve_path(&vdst);
    if !storage.same_backend(&vsrc, &vdst) {
        let buf = src_op.read(&src_remote).await.map_err(|e| e.to_string())?;
        dst_op
            .write(&dst_remote, buf)
            .await
            .map_err(|e| e.to_string())?;
        src_op
            .delete(&src_remote)
            .await
            .map_err(|e| e.to_string())?;
    } else {
        src_op
            .rename(&src_remote, &dst_remote)
            .await
            .map_err(|e| e.to_string())?;
    }

    Ok(AgentToolResult::success_json(serde_json::json!({
        "src": raw_src,
        "dst": raw_dst,
        "moved": true,
    })))
}
