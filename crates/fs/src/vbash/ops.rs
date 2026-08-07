//! Operation implementations for the VFS bash tool.
//!
//! Every function takes an [`opendal::Operator`] reference and returns a
//! [`ToolResult`]. No subprocess is spawned anywhere in this module.

use agentik_core::tools::{ToolError, ToolResult};
use agentik_sdk::types::{ToolImageSource, ToolResult as AgentToolResult, ToolResultBlock};
use base64::Engine as _;
use futures::StreamExt;

use crate::storage::OpendalFileStorage;

/// Default maximum lines for `read` when no explicit `limit` is given.
const DEFAULT_MAX_LINES: usize = 2000;
/// Image extensions recognised by the `read` op.
const IMAGE_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "gif", "webp"];
/// Number of leading bytes sampled for binary detection.
const BINARY_SAMPLE_SIZE: usize = 8192;
/// A file is treated as binary when more than this fraction of the sampled
/// bytes are non-text control characters.
const BINARY_CONTROL_RATIO: f32 = 0.30;

// ══════════════════ reading ops ══════════════════

/// `read` — rich read with line numbers (text), base64 (image), or formatted (notebook).
pub async fn op_read(
    op: &opendal::Operator,
    path: Option<&str>,
    offset: Option<usize>,
    limit: Option<usize>,
) -> Result<AgentToolResult, ToolError> {
    let raw_path = path.ok_or("missing 'path' for read")?;
    let vpath = OpendalFileStorage::normalize_path(raw_path);

    let ext = std::path::Path::new(raw_path)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase())
        .unwrap_or_default();

    if ext == "ipynb" {
        return read_notebook(op, &vpath, raw_path).await;
    }
    if IMAGE_EXTENSIONS.contains(&ext.as_str()) {
        return read_image(op, &vpath, raw_path, &ext).await;
    }
    read_text_numbered(op, &vpath, raw_path, offset, limit).await
}

/// `cat` — plain text read, no line numbers, no special formatting.
pub async fn op_cat(
    op: &opendal::Operator,
    path: Option<&str>,
    offset: Option<usize>,
    limit: Option<usize>,
) -> Result<AgentToolResult, ToolError> {
    let raw_path = path.ok_or("missing 'path' for cat")?;
    let vpath = OpendalFileStorage::normalize_path(raw_path);

    let meta = match op.stat(&vpath).await {
        Ok(m) => m,
        Err(e) if matches!(e.kind(), opendal::ErrorKind::NotFound) => {
            return Ok(AgentToolResult::error(format!(
                "cat: '{raw_path}': No such file or directory"
            )));
        }
        Err(e) => return Ok(AgentToolResult::error(format!("cat: '{raw_path}': {e}"))),
    };
    if meta.is_dir() {
        return Ok(AgentToolResult::error(format!(
            "cat: '{raw_path}' is a directory. Use 'ls' or 'tree' to list its contents."
        )));
    }
    let total_size = meta.content_length();
    if total_size == 0 {
        return Ok(AgentToolResult::success_json(serde_json::json!({
            "path": raw_path,
            "content": "(file is empty)",
            "total_size": 0,
        })));
    }

    let buf = op.read(&vpath).await.map_err(|e| e.to_string())?;
    let bytes = buf.to_vec();
    if is_binary(&bytes) {
        return Ok(binary_refused(raw_path, bytes.len() as u64));
    }
    let content = String::from_utf8_lossy(&bytes);

    let lines: Vec<&str> = content.lines().collect();
    let total_lines = lines.len();

    let start = offset.unwrap_or(1).saturating_sub(1).min(total_lines);
    let end = match limit {
        Some(n) => (start + n).min(total_lines),
        None => total_lines,
    };

    let out: String = lines[start..end].join("\n");

    Ok(AgentToolResult::success_json(serde_json::json!({
        "path": raw_path,
        "content": out,
        "start_line": start + 1,
        "lines_returned": end.saturating_sub(start),
        "total_lines": total_lines,
    })))
}

/// `head` — first N lines (default 10).
///
/// `offset` (1-indexed) skips that many leading lines before applying
/// `limit`. Mirrors `cat` / `read` semantics so the three ops stay
/// interchangeable for paging through a file.
pub async fn op_head(
    op: &opendal::Operator,
    path: Option<&str>,
    offset: Option<usize>,
    limit: Option<usize>,
) -> Result<AgentToolResult, ToolError> {
    let raw_path = path.ok_or("missing 'path' for head")?;
    let vpath = OpendalFileStorage::normalize_path(raw_path);
    let n = limit.unwrap_or(10);
    let start = offset.unwrap_or(1).saturating_sub(1);

    let meta = match op.stat(&vpath).await {
        Ok(m) => m,
        Err(e) if matches!(e.kind(), opendal::ErrorKind::NotFound) => {
            return Ok(AgentToolResult::error(format!(
                "head: '{raw_path}': No such file or directory"
            )));
        }
        Err(e) => return Ok(AgentToolResult::error(format!("head: '{raw_path}': {e}"))),
    };
    if meta.is_dir() {
        return Ok(AgentToolResult::error(format!(
            "head: '{raw_path}' is a directory. Use 'ls' or 'tree' to list its contents."
        )));
    }

    let buf = op.read(&vpath).await.map_err(|e| e.to_string())?;
    let bytes = buf.to_vec();
    if is_binary(&bytes) {
        return Ok(binary_refused(raw_path, bytes.len() as u64));
    }
    let content = String::from_utf8_lossy(&bytes);
    let lines: Vec<&str> = content.lines().collect();
    let total_lines = lines.len();
    let start = start.min(total_lines);
    let end = (start + n).min(total_lines);
    let out: String = lines[start..end].join("\n");

    Ok(AgentToolResult::success_json(serde_json::json!({
        "path": raw_path,
        "content": out,
        "start_line": start + 1,
        "lines_returned": end - start,
        "total_lines": total_lines,
    })))
}

/// `tail` — last N lines (default 10).
///
/// `offset` is **not** meaningful for `tail` (Unix tail has no offset
/// concept). If the caller passes one it is silently ignored and the
/// response sets `offset_ignored: true` so the caller can detect the
/// mismatch and switch to `cat` / `read` if they meant to page.
pub async fn op_tail(
    op: &opendal::Operator,
    path: Option<&str>,
    offset: Option<usize>,
    limit: Option<usize>,
) -> Result<AgentToolResult, ToolError> {
    let raw_path = path.ok_or("missing 'path' for tail")?;
    let vpath = OpendalFileStorage::normalize_path(raw_path);
    let n = limit.unwrap_or(10);

    let meta = match op.stat(&vpath).await {
        Ok(m) => m,
        Err(e) if matches!(e.kind(), opendal::ErrorKind::NotFound) => {
            return Ok(AgentToolResult::error(format!(
                "tail: '{raw_path}': No such file or directory"
            )));
        }
        Err(e) => return Ok(AgentToolResult::error(format!("tail: '{raw_path}': {e}"))),
    };
    if meta.is_dir() {
        return Ok(AgentToolResult::error(format!(
            "tail: '{raw_path}' is a directory. Use 'ls' or 'tree' to list its contents."
        )));
    }

    let buf = op.read(&vpath).await.map_err(|e| e.to_string())?;
    let bytes = buf.to_vec();
    if is_binary(&bytes) {
        return Ok(binary_refused(raw_path, bytes.len() as u64));
    }
    let content = String::from_utf8_lossy(&bytes);
    let lines: Vec<&str> = content.lines().collect();
    let total = lines.len();
    let start = total.saturating_sub(n);

    let out: String = lines[start..].join("\n");

    let mut payload = serde_json::json!({
        "path": raw_path,
        "content": out,
        "lines_returned": total - start,
        "total_lines": total,
    });
    if offset.is_some() {
        payload["offset_ignored"] = serde_json::json!(true);
    }

    Ok(AgentToolResult::success_json(payload))
}

// ══════════════════ writing ops ══════════════════

/// `write` — create or overwrite a file.
///
/// The implementation performs an explicit delete-then-write to sidestep
/// the OpenDAL Fs backend race that produces `writer got too little data`
/// when an existing file is overwritten with a longer payload. After the
/// write we retry-stat to defend against the same backend's brief
/// final-consistency window where a freshly written file is not yet
/// visible to a follow-up stat.
pub async fn op_write(
    op: &opendal::Operator,
    path: Option<&str>,
    content: Option<&str>,
) -> Result<AgentToolResult, ToolError> {
    let raw_path = path.ok_or("missing 'path' for write")?;
    let content = content.unwrap_or("").to_string();
    let vpath = OpendalFileStorage::normalize_path(raw_path);
    let size = content.len() as u64;

    // Atomic-replace path: drop any existing entry first so the Fs
    // backend's overwrite path cannot race with its size bookkeeping.
    if let Err(e) = op.delete(&vpath).await {
        if !matches!(e.kind(), opendal::ErrorKind::NotFound) {
            return Ok(AgentToolResult::error(format!(
                "write: failed to clear {raw_path}: {e}"
            )));
        }
    }

    op.write(&vpath, content.into_bytes())
        .await
        .map_err(|e| e.to_string())?;

    // Visibility probe: the Fs backend occasionally returns a stale
    // NotFound on the stat that immediately follows a write in the same
    // process. Retry a handful of times before declaring a real failure.
    for attempt in 0..5 {
        if op.stat(&vpath).await.is_ok() {
            return Ok(AgentToolResult::success_json(serde_json::json!({
                "path": raw_path,
                "size": size,
            })));
        }
        tokio::time::sleep(std::time::Duration::from_millis(
            5 * (attempt as u64 + 1),
        ))
        .await;
    }

    Ok(AgentToolResult::error(format!(
        "write: wrote {size} bytes to {raw_path} but the file is not yet visible to a follow-up stat. \
         The backend may be in an inconsistent state; retry or verify with a separate tool call."
    )))
}

/// `edit` — exact string replacement.
///
/// All error paths return `Ok(ToolResult::error(...))` (never `Err`) so the
/// LLM always receives a structured result.  Mirrors the behaviour of the
/// original `agentik-tools` `EditTool`.
pub async fn op_edit(
    op: &opendal::Operator,
    path: Option<&str>,
    old_string: Option<&str>,
    new_string: Option<&str>,
    replace_all: Option<bool>,
) -> Result<AgentToolResult, ToolError> {
    let raw_path = match path {
        Some(p) => p,
        None => return Ok(AgentToolResult::error("missing 'path' for edit")),
    };
    let old = match old_string {
        Some(s) => s,
        None => return Ok(AgentToolResult::error("missing 'old_string' for edit")),
    };
    let new = match new_string {
        Some(s) => s,
        None => return Ok(AgentToolResult::error("missing 'new_string' for edit")),
    };

    if old == new {
        return Ok(AgentToolResult::error(
            "old_string must differ from new_string",
        ));
    }

    let vpath = OpendalFileStorage::normalize_path(raw_path);

    // Read current content.
    let buf = match op.read(&vpath).await {
        Ok(b) => b,
        Err(e) => {
            return Ok(AgentToolResult::error(format!(
                "Failed to read {raw_path}: {e}"
            )));
        }
    };
    let text = match String::from_utf8(buf.to_vec()) {
        Ok(s) => s,
        Err(_) => {
            return Ok(AgentToolResult::error(format!(
                "File {raw_path} is not valid UTF-8"
            )));
        }
    };

    let replace_all = replace_all.unwrap_or(false);
    let count = text.matches(old).count();

    if count == 0 {
        return Ok(AgentToolResult::error("old_string not found in file"));
    }
    if count > 1 && !replace_all {
        return Ok(AgentToolResult::error(format!(
            "old_string matches {count} locations; set replace_all=true or make old_string unique"
        )));
    }

    let new_text = if replace_all {
        text.replace(old, new)
    } else {
        text.replacen(old, new, 1)
    };

    if let Err(e) = op.write(&vpath, new_text.into_bytes()).await {
        return Ok(AgentToolResult::error(format!(
            "Failed to write {raw_path}: {e}"
        )));
    }

    let n = if replace_all { count as u64 } else { 1 };
    Ok(AgentToolResult::success_json(serde_json::json!({
        "path": raw_path,
        "replacements": n,
    })))
}

/// `touch` — create empty file if it doesn't exist; no-op if it does
/// (OpenDAL cannot set mtime).
pub async fn op_touch(
    op: &opendal::Operator,
    path: Option<&str>,
) -> Result<AgentToolResult, ToolError> {
    let raw_path = path.ok_or("missing 'path' for touch")?;
    let vpath = OpendalFileStorage::normalize_path(raw_path);

    let exists = op.stat(&vpath).await.is_ok();
    if !exists {
        op.write(&vpath, "").await.map_err(|e| e.to_string())?;
    }

    Ok(AgentToolResult::success_json(serde_json::json!({
        "path": raw_path,
        "created": !exists,
    })))
}

// ══════════════════ filesystem ops ══════════════════

/// Default maximum entries returned by `ls` when no explicit `limit` is given.
///
/// Kept modest so a single listing does not flood the agent's context
/// window. Callers may raise it via `limit`, or page with `offset`.
const DEFAULT_LS_LIMIT: usize = 200;

/// `ls` — list directory entries.
///
/// Results are paginated: `offset` entries are skipped, then up to `limit`
/// entries (default [`DEFAULT_LS_LIMIT`]) are collected. The response
/// includes a `truncated` flag and a `next_offset` hint so the caller can
/// continue listing when more entries remain.
pub async fn op_ls(
    op: &opendal::Operator,
    path: Option<&str>,
    recursive: Option<bool>,
    limit: Option<usize>,
    offset: Option<usize>,
) -> Result<AgentToolResult, ToolError> {
    let vpath = OpendalFileStorage::normalize_path(path.unwrap_or("/"));
    // Match Unix `ls` (non-recursive by default); pass recursive=true
    // for the previous recursive listing behaviour.
    let recursive = recursive.unwrap_or(false);
    // Treat 0 as "no explicit limit" — the field is optional in the schema
    // and some callers default-empty rather than omitting it.
    let max = limit.unwrap_or(DEFAULT_LS_LIMIT).max(1);
    let skip = offset.unwrap_or(0);

    let mut lister = if recursive {
        op.lister_with(&vpath).recursive(true).await
    } else {
        // Fs backend needs trailing '/' for non-recursive listing
        let scan = if vpath.ends_with('/') {
            vpath.clone()
        } else {
            format!("{vpath}/")
        };
        op.lister_with(&scan).recursive(false).await
    }
    .map_err(|e| e.to_string())?;

    // OpenDAL's Fs backend yields the scan root itself as the first entry
    // (e.g. listing "/" includes "/" as a directory entry). We strip the
    // trailing slash for comparison so it matches both "/" and "/sub/".
    let scan_root = vpath.trim_end_matches('/').to_string();

    let mut items: Vec<serde_json::Value> = Vec::with_capacity(max.min(1024));
    // Track the index of the current entry across the whole stream so we
    // can implement offset pagination.
    let mut idx = 0usize;
    let mut truncated = false;

    while let Some(entry) = lister.next().await {
        let entry = entry.map_err(|e| e.to_string())?;

        // Skip the scan-root self-entry (see comment above).
        let entry_path_raw = entry.path().to_string();
        if entry.metadata().is_dir() && entry_path_raw.trim_end_matches('/') == scan_root {
            continue;
        }

        idx += 1;

        // Skip entries before the requested offset.
        if idx <= skip {
            continue;
        }

        // Stop once we have filled the page. We still report `truncated`
        // because this entry existed and was refused — there is at least
        // one more entry beyond the current page.
        if items.len() >= max {
            truncated = true;
            break;
        }

        let entry_path = entry.path().to_string();
        let meta = entry.metadata();
        let is_dir = meta.is_dir();
        let size = if is_dir {
            0
        } else {
            op.stat(&entry_path)
                .await
                .ok()
                .map(|m| m.content_length())
                .unwrap_or(meta.content_length())
        };
        items.push(serde_json::json!({
            "name": entry_path,
            "is_dir": is_dir,
            "size": size,
        }));
    }

    // `truncated` is only set inside the loop when an entry was refused
    // after the page was already full. If the stream ended naturally with
    // exactly `max` entries, `truncated` stays false — that is the
    // non-truncated boundary case.

    let returned = items.len();
    let next_offset = if truncated {
        Some(skip + returned + 1)
    } else {
        None
    };

    let mut payload = serde_json::json!({
        "path": vpath,
        "entries": items,
        "returned": returned,
        "truncated": truncated,
    });
    if let Some(no) = next_offset {
        payload["next_offset"] = serde_json::json!(no);
    }

    Ok(AgentToolResult::success_json(payload))
}

/// `stat` — file metadata.
pub async fn op_stat(
    op: &opendal::Operator,
    path: Option<&str>,
) -> Result<AgentToolResult, ToolError> {
    let raw_path = path.ok_or("missing 'path' for stat")?;
    let vpath = OpendalFileStorage::normalize_path(raw_path);
    let meta = match op.stat(&vpath).await {
        Ok(m) => m,
        Err(_) => {
            return Ok(AgentToolResult::error(format!(
                "File or directory does not exist: {raw_path}"
            )));
        }
    };

    Ok(AgentToolResult::success_json(serde_json::json!({
        "path": raw_path,
        "is_dir": meta.is_dir(),
        "size": meta.content_length(),
        "last_modified": meta.last_modified().map(|t| t.to_string()).unwrap_or_default(),
    })))
}

/// `mkdir` — create a directory.
pub async fn op_mkdir(
    op: &opendal::Operator,
    path: Option<&str>,
) -> Result<AgentToolResult, ToolError> {
    let raw_path = path.ok_or("missing 'path' for mkdir")?;
    let vpath = OpendalFileStorage::normalize_path(raw_path);

    // OpenDAL Fs backend requires a trailing '/' for directory creation.
    let dir_path = if vpath.ends_with('/') {
        vpath.clone()
    } else {
        format!("{vpath}/")
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
    op: &opendal::Operator,
    path: Option<&str>,
    recursive: Option<bool>,
) -> Result<AgentToolResult, ToolError> {
    let raw_path = path.ok_or("missing 'path' for rm")?;
    let vpath = OpendalFileStorage::normalize_path(raw_path);

    if vpath == "/" {
        return Ok(AgentToolResult::error(
            "Refusing to rm '/': that is the virtual filesystem root. \
             Use a sub-path like '/tmp' instead.",
        ));
    }

    let meta = match op.stat(&vpath).await {
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
        return match op.delete(&vpath).await {
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

    op.delete_with(&vpath)
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
    op: &opendal::Operator,
    src: Option<&str>,
    dst: Option<&str>,
) -> Result<AgentToolResult, ToolError> {
    let raw_src = src.ok_or("missing 'src' for cp")?;
    let raw_dst = dst.ok_or("missing 'dst' for cp")?;
    let vsrc = OpendalFileStorage::normalize_path(raw_src);
    let vdst = OpendalFileStorage::normalize_path(raw_dst);

    op.copy(&vsrc, &vdst).await.map_err(|e| e.to_string())?;

    Ok(AgentToolResult::success_json(serde_json::json!({
        "src": raw_src,
        "dst": raw_dst,
        "copied": true,
    })))
}

/// `mv` — rename/move a file.
pub async fn op_mv(
    op: &opendal::Operator,
    src: Option<&str>,
    dst: Option<&str>,
) -> Result<AgentToolResult, ToolError> {
    let raw_src = src.ok_or("missing 'src' for mv")?;
    let raw_dst = dst.ok_or("missing 'dst' for mv")?;
    let vsrc = OpendalFileStorage::normalize_path(raw_src);
    let vdst = OpendalFileStorage::normalize_path(raw_dst);

    op.rename(&vsrc, &vdst).await.map_err(|e| e.to_string())?;

    Ok(AgentToolResult::success_json(serde_json::json!({
        "src": raw_src,
        "dst": raw_dst,
        "moved": true,
    })))
}

/// `wc` — count lines, words, and bytes.
pub async fn op_wc(
    op: &opendal::Operator,
    path: Option<&str>,
) -> Result<AgentToolResult, ToolError> {
    let raw_path = path.ok_or("missing 'path' for wc")?;
    let vpath = OpendalFileStorage::normalize_path(raw_path);

    let buf = op.read(&vpath).await.map_err(|e| e.to_string())?;
    let bytes = buf.len();
    let data = buf.to_vec();
    let text = String::from_utf8_lossy(&data);
    let lines = text.lines().count();
    let words = text.split_whitespace().count();

    Ok(AgentToolResult::success_json(serde_json::json!({
        "path": raw_path,
        "lines": lines,
        "words": words,
        "bytes": bytes,
    })))
}

/// `tree` — formatted recursive directory listing using standard
/// `├──` / `└──` / `│` tree characters (instead of plain indented
/// slashes which were hard to scan visually).
pub async fn op_tree(
    op: &opendal::Operator,
    path: Option<&str>,
    limit: Option<usize>,
) -> Result<AgentToolResult, ToolError> {
    let vpath = OpendalFileStorage::normalize_path(path.unwrap_or("/"));
    let max_entries = limit.unwrap_or(500);

    // OpenDAL Fs backend requires a trailing '/' to walk recursively.
    let scan = if vpath.ends_with('/') {
        vpath.clone()
    } else {
        format!("{vpath}/")
    };
    let mut lister = op
        .lister_with(&scan)
        .recursive(true)
        .await
        .map_err(|e| e.to_string())?;

    // Collect entries as (depth, path, is_dir). Drop the scan-root
    // self-entry to avoid duplicating it in the rendered tree.
    let mut entries: Vec<(usize, String, bool)> = Vec::new();
    let prefix = vpath.trim_end_matches('/');
    let scan_root = if prefix.is_empty() { "/" } else { prefix };
    while let Some(entry) = lister.next().await {
        let entry = entry.map_err(|e| e.to_string())?;
        let p = entry.path().to_string();
        let is_dir = entry.metadata().is_dir();

        if is_dir && p.trim_end_matches('/') == scan_root.trim_end_matches('/') {
            continue;
        }

        let rel = if prefix.is_empty() {
            p.as_str()
        } else {
            p.strip_prefix(prefix).unwrap_or(&p).trim_start_matches('/')
        };
        let depth = rel.matches('/').count();
        entries.push((depth, p, is_dir));
        if entries.len() >= max_entries {
            break;
        }
    }

    entries.sort_by(|a, b| a.1.cmp(&b.1));

    // Index immediate children by parent path. Top-level entries
    // (depth == 1 relative to the scan root) are always parented to '/'.
    // OpenDAL returns each top-level directory as a trailing-slash
    // entry like 'd/', so computing parent from `rfind('/')` would
    // wrongly assign `d/` as a child of `d` (its own directory name).
    // Using the recorded depth sidesteps that.
    let mut children_of: std::collections::BTreeMap<String, Vec<String>> =
        std::collections::BTreeMap::new();
    children_of.insert("/".to_string(), Vec::new());
    for (depth, p, _) in &entries {
        let parent = if *depth == 1 {
            "/".to_string()
        } else if let Some(idx) = p.rfind('/') {
            if idx == 0 {
                "/".to_string()
            } else {
                p[..idx].to_string()
            }
        } else {
            "/".to_string()
        };
        children_of.entry(parent).or_default().push(p.clone());
    }
    for v in children_of.values_mut() {
        v.sort();
    }

    let meta_by_path: std::collections::HashMap<String, bool> =
        entries.iter().map(|(_, p, d)| (p.clone(), *d)).collect();


    // Collect all entry paths and the parent-directory path each one
    // belongs to. The walker below uses these to render the tree.
    let all_paths: Vec<String> = entries.iter().map(|(_, p, _)| p.clone()).collect();

    fn walk(
        parent: &str,
        ancestor_has_more: &mut Vec<bool>,
        all_paths: &[String],
        meta_by_path: &std::collections::HashMap<String, bool>,
        out: &mut String,
    ) {
        // The `parent` for the root is '/'; for a directory it's the
        // directory path (e.g. 'd'). A child of `parent` is any entry
        // whose immediate parent directory is `parent`. For a directory
        // entry like `d/`, its immediate children are all entries with
        // path `parent/segment` (depth relative to scan root = depth of
        // parent + 1).
        let mut children: Vec<&String> = all_paths
            .iter()
            .filter(|p| {
                if parent == "/" {
                    // Top-level: the first path segment is non-empty.
                    // OpenDAL returns directories with a trailing slash
                    // (e.g. `d/`), so trim it before checking for
                    // further segments.
                    let trimmed = p.trim_start_matches('/').trim_end_matches('/');
                    !trimmed.is_empty() && !trimmed.contains('/')
                } else {
                    // Entry belongs to `parent` if it is exactly one
                    // path segment deeper (e.g. parent='d' matches
                    // 'd/a.txt' but not 'd/a/b.txt'). The directory
                    // entry 'd/' is filtered here too — `p` may have a
                    // trailing slash and equal `parent + '/'`, in which
                    // case it represents `parent` itself and must not
                    // be re-descended into.
                    let trimmed = p.trim_start_matches('/').trim_end_matches('/');
                    let parent_trimmed = parent.trim_start_matches('/').trim_end_matches('/');
                    trimmed != parent_trimmed
                        && trimmed.starts_with(&format!("{}/", parent_trimmed))
                        && !trimmed[parent_trimmed.len() + 1..].contains('/')
                }
            })
            .collect();
        children.sort();
            if children.is_empty() {
            return;
        }
        let n = children.len();
        for (i, child) in children.iter().enumerate() {
            let is_last = i + 1 == n;
            let child_path = child.as_str();
            let is_dir = meta_by_path.get(*child).copied().unwrap_or(false);
            // Display name: the last path segment, with '/' for dirs.
            // We trim trailing '/' so a directory entry like 'd/'
            // produces the segment name 'd' rather than an empty string.
            let trimmed = child_path.trim_end_matches('/');
            let seg = trimmed.rsplit('/').next().unwrap_or(trimmed);
            let display = if is_dir {
                format!("{seg}/")
            } else {
                seg.to_string()
            };
            let connector = if is_last { "└── " } else { "├── " };
            for more in ancestor_has_more.iter() {
                out.push_str(if *more { "│   " } else { "    " });
            }
            out.push_str(connector);
            out.push_str(&display);
            out.push('\n');

            if is_dir {
                ancestor_has_more.push(!is_last);
                // Recurse with the directory path (no trailing slash)
                // so subsequent prefix lookups match child entries.
                walk(child.trim_end_matches('/'), ancestor_has_more, &all_paths, &meta_by_path, out);
                ancestor_has_more.pop();
            }
        }
    }

    let mut out = String::new();
    out.push_str(&vpath);
    out.push('\n');

    let mut ancestor_has_more: Vec<bool> = Vec::new();
    walk("/", &mut ancestor_has_more, &all_paths, &meta_by_path, &mut out);

    Ok(AgentToolResult::success_json(serde_json::json!({
        "path": vpath,
        "content": out,
        "entries": entries.len(),
        "_dbg_co": format!("{:?}", children_of),
        "_dbg_meta": format!("{:?}", meta_by_path),
    })))
}

// ══════════════════ private helpers ══════════════════

/// Text read with line numbers (cat -n style).
async fn read_text_numbered(
    op: &opendal::Operator,
    vpath: &str,
    display_path: &str,
    offset: Option<usize>,
    limit: Option<usize>,
) -> Result<AgentToolResult, ToolError> {
    let meta = match op.stat(vpath).await {
        Ok(m) => m,
        Err(e) if matches!(e.kind(), opendal::ErrorKind::NotFound) => {
            return Ok(AgentToolResult::error(format!(
                "read: '{display_path}': No such file or directory"
            )));
        }
        Err(e) => return Ok(AgentToolResult::error(format!("read: '{display_path}': {e}"))),
    };
    if meta.is_dir() {
        return Ok(AgentToolResult::error(format!(
            "read: '{display_path}' is a directory. Use 'ls' or 'tree' to list its contents."
        )));
    }
    let total_size = meta.content_length();
    if total_size == 0 {
        return Ok(AgentToolResult::success_json(serde_json::json!({
            "path": display_path,
            "content": "(file exists but is empty)",
            "total_size": 0,
        })));
    }

    let reader = op.reader(vpath).await.map_err(|e| e.to_string())?;
    let buf = reader
        .read(0..total_size)
        .await
        .map_err(|e| e.to_string())?;
    let data = buf.to_vec();
    if is_binary(&data) {
        return Ok(binary_refused(display_path, total_size));
    }
    let content = String::from_utf8_lossy(&data);

    let lines: Vec<&str> = content.lines().collect();
    let total_lines = lines.len();

    let start = offset.unwrap_or(1).saturating_sub(1).min(total_lines);
    let end = match limit {
        Some(n) => (start + n).min(total_lines),
        None => (start + DEFAULT_MAX_LINES).min(total_lines),
    };

    let width = end.to_string().len().max(3);
    let mut out = String::new();
    for (i, line) in lines[start..end].iter().enumerate() {
        let n = start + i + 1;
        out.push_str(&format!("{n:>width$}\t{line}\n"));
    }

    let truncated = end < total_lines;
    if limit.is_none() && truncated {
        let remaining = total_lines - end;
        out.push_str(&format!(
            "\n... ({remaining} more lines, use offset={} to continue)\n",
            end + 1
        ));
    }

    Ok(AgentToolResult::success_json(serde_json::json!({
        "path": display_path,
        "content": out,
        "start_line": start + 1,
        "lines_read": end - start,
        "total_lines": total_lines,
        "total_size": total_size,
        "truncated": truncated,
    })))
}

/// Image read → base64 image block.
async fn read_image(
    op: &opendal::Operator,
    vpath: &str,
    display_path: &str,
    ext: &str,
) -> Result<AgentToolResult, ToolError> {
    let total_size = op
        .stat(vpath)
        .await
        .map(|m| m.content_length())
        .unwrap_or(0);
    if total_size == 0 {
        return Ok(AgentToolResult::error(format!(
            "Image file is empty: {display_path}"
        )));
    }

    let reader = op.reader(vpath).await.map_err(|e| e.to_string())?;
    let buf = reader
        .read(0..total_size)
        .await
        .map_err(|e| e.to_string())?;

    let media_type = image_media_type(ext);
    let data = base64::engine::general_purpose::STANDARD.encode(&buf.to_vec());

    Ok(AgentToolResult::with_blocks(vec![ToolResultBlock::Image {
        source: ToolImageSource::Base64 {
            media_type: media_type.to_string(),
            data,
        },
    }]))
}

fn image_media_type(ext: &str) -> &'static str {
    match ext {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        _ => "application/octet-stream",
    }
}

/// Notebook (.ipynb) read → formatted text.
async fn read_notebook(
    op: &opendal::Operator,
    vpath: &str,
    display_path: &str,
) -> Result<AgentToolResult, ToolError> {
    let total_size = op
        .stat(vpath)
        .await
        .map(|m| m.content_length())
        .unwrap_or(0);
    if total_size == 0 {
        return Ok(AgentToolResult::error(format!(
            "Notebook file is empty: {display_path}"
        )));
    }

    let reader = op.reader(vpath).await.map_err(|e| e.to_string())?;
    let buf = reader
        .read(0..total_size)
        .await
        .map_err(|e| e.to_string())?;
    let data = buf.to_vec();
    let raw = String::from_utf8_lossy(&data);

    let nb: serde_json::Value = match serde_json::from_str(&raw) {
        Ok(v) => v,
        Err(e) => {
            return Ok(AgentToolResult::error(format!(
                "Invalid notebook JSON in {display_path}: {e}"
            )));
        }
    };

    let cells = nb.get("cells").and_then(|c| c.as_array());
    let cell_count = cells.map_or(0, |c| c.len());

    let mut out = String::new();
    out.push_str(&format!(
        "Jupyter notebook: {display_path}\n{cell_count} cells\n\n"
    ));

    if let Some(cells) = cells {
        for (idx, cell) in cells.iter().enumerate() {
            let cell_type = cell
                .get("cell_type")
                .and_then(|t| t.as_str())
                .unwrap_or("unknown");
            out.push_str(&format!("─ Cell {} [{cell_type}] ─\n", idx + 1));

            if let Some(source) = cell.get("source") {
                let text = extract_source(source);
                out.push_str(&text);
                if !text.ends_with('\n') {
                    out.push('\n');
                }
            }

            if cell_type == "code" {
                if let Some(outputs) = cell.get("outputs").and_then(|o| o.as_array()) {
                    for output in outputs {
                        if let Some(text) = output.get("text").and_then(|t| t.as_str()) {
                            out.push_str("[output]\n");
                            out.push_str(text);
                            if !text.ends_with('\n') {
                                out.push('\n');
                            }
                        } else if let Some(data) = output
                            .get("data")
                            .and_then(|d| d.get("text/plain"))
                            .and_then(|t| t.as_str())
                        {
                            out.push_str("[output]\n");
                            out.push_str(data);
                            out.push('\n');
                        }
                    }
                }
            }
            out.push('\n');
        }
    }

    Ok(AgentToolResult::success_json(serde_json::json!({
        "path": display_path,
        "content": out,
        "cell_count": cell_count,
        "total_size": total_size,
    })))
}

/// `.ipynb` `source` can be a string or an array of strings.
fn extract_source(source: &serde_json::Value) -> String {
    if let Some(s) = source.as_str() {
        return s.to_string();
    }
    if let Some(arr) = source.as_array() {
        return arr
            .iter()
            .filter_map(|s| s.as_str())
            .collect::<Vec<_>>()
            .join("");
    }
    String::new()
}

// ══════════════════ binary detection ══════════════════

/// Heuristic check for whether a byte slice is binary (non-text) data.
///
/// Mirrors the approach used by git and ripgrep: sample the leading bytes
/// and declare binary if either
/// 1. a NUL byte (`\0`) is present — a strong binary signal, or
/// 2. more than [`BINARY_CONTROL_RATIO`] of the sampled bytes are
///    non-text control characters (anything below `0x20` other than the
///    common whitespace bytes `\t`, `\n`, `\r`, plus `0x7f` DEL).
///
/// High bytes (`>= 0x80`) are intentionally ignored so that valid UTF-8
/// multibyte content (CJK, emoji, etc.) is not misclassified.
pub(crate) fn is_binary(bytes: &[u8]) -> bool {
    let sample = &bytes[..bytes.len().min(BINARY_SAMPLE_SIZE)];
    if sample.is_empty() {
        return false;
    }
    if sample.contains(&0u8) {
        return true;
    }
    let non_text = sample
        .iter()
        .filter(|&&b| (b < 0x20 && b != b'\t' && b != b'\n' && b != b'\r') || b == 0x7f)
        .count();
    let ratio = non_text as f32 / sample.len() as f32;
    ratio > BINARY_CONTROL_RATIO
}

/// Build a structured "refused: binary" tool result so the agent gets a
/// clear explanation instead of garbled `from_utf8_lossy` output.
fn binary_refused(display_path: &str, size: u64) -> AgentToolResult {
    AgentToolResult::error(format!(
        "Refused to read '{display_path}': the file appears to be binary \
         ({size} bytes, non-text content detected). Reading it as text would \
         produce garbled output; use a dedicated tool to inspect binary data."
    ))
}
