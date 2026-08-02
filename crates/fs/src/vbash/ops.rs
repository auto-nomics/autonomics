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

    let total_size = op
        .stat(&vpath)
        .await
        .map(|m| m.content_length())
        .unwrap_or(0);
    if total_size == 0 {
        return Ok(AgentToolResult::success_json(serde_json::json!({
            "path": raw_path,
            "content": "(file is empty)",
            "total_size": 0,
        })));
    }

    let buf = op.read(&vpath).await.map_err(|e| e.to_string())?;
    let bytes = buf.to_vec();
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
pub async fn op_head(
    op: &opendal::Operator,
    path: Option<&str>,
    limit: Option<usize>,
) -> Result<AgentToolResult, ToolError> {
    let raw_path = path.ok_or("missing 'path' for head")?;
    let vpath = OpendalFileStorage::normalize_path(raw_path);
    let n = limit.unwrap_or(10);

    let buf = op.read(&vpath).await.map_err(|e| e.to_string())?;
    let bytes = buf.to_vec();
    let content = String::from_utf8_lossy(&bytes);
    let lines: Vec<&str> = content.lines().collect();
    let take = n.min(lines.len());

    let out: String = lines[..take].join("\n");

    Ok(AgentToolResult::success_json(serde_json::json!({
        "path": raw_path,
        "content": out,
        "lines_returned": take,
        "total_lines": lines.len(),
    })))
}

/// `tail` — last N lines (default 10).
pub async fn op_tail(
    op: &opendal::Operator,
    path: Option<&str>,
    limit: Option<usize>,
) -> Result<AgentToolResult, ToolError> {
    let raw_path = path.ok_or("missing 'path' for tail")?;
    let vpath = OpendalFileStorage::normalize_path(raw_path);
    let n = limit.unwrap_or(10);

    let buf = op.read(&vpath).await.map_err(|e| e.to_string())?;
    let bytes = buf.to_vec();
    let content = String::from_utf8_lossy(&bytes);
    let lines: Vec<&str> = content.lines().collect();
    let total = lines.len();
    let start = total.saturating_sub(n);

    let out: String = lines[start..].join("\n");

    Ok(AgentToolResult::success_json(serde_json::json!({
        "path": raw_path,
        "content": out,
        "lines_returned": total - start,
        "total_lines": total,
    })))
}

// ══════════════════ writing ops ══════════════════

/// `write` — create or overwrite a file.
pub async fn op_write(
    op: &opendal::Operator,
    path: Option<&str>,
    content: Option<&str>,
) -> Result<AgentToolResult, ToolError> {
    let raw_path = path.ok_or("missing 'path' for write")?;
    let content = content.unwrap_or("").to_string();
    let vpath = OpendalFileStorage::normalize_path(raw_path);
    let size = content.len() as u64;

    op.write(&vpath, content.into_bytes())
        .await
        .map_err(|e| e.to_string())?;

    Ok(AgentToolResult::success_json(serde_json::json!({
        "path": raw_path,
        "size": size,
    })))
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

/// `ls` — list directory entries.
pub async fn op_ls(
    op: &opendal::Operator,
    path: Option<&str>,
    recursive: Option<bool>,
    limit: Option<usize>,
) -> Result<AgentToolResult, ToolError> {
    let vpath = OpendalFileStorage::normalize_path(path.unwrap_or("/"));
    let recursive = recursive.unwrap_or(true);
    let max = limit.unwrap_or(1000);

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

    let mut items = Vec::new();
    while let Some(entry) = lister.next().await {
        let entry = entry.map_err(|e| e.to_string())?;
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
        if items.len() >= max {
            break;
        }
    }

    Ok(AgentToolResult::success_json(serde_json::json!({
        "path": vpath,
        "entries": items,
    })))
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

/// `rm` — delete a file or directory (recursive).
#[allow(deprecated)]
pub async fn op_rm(
    op: &opendal::Operator,
    path: Option<&str>,
) -> Result<AgentToolResult, ToolError> {
    let raw_path = path.ok_or("missing 'path' for rm")?;
    let vpath = OpendalFileStorage::normalize_path(raw_path);

    // delete_with().recursive(true) is the recommended API (remove_all is deprecated).
    op.delete_with(&vpath)
        .recursive(true)
        .await
        .map_err(|e| e.to_string())?;

    Ok(AgentToolResult::success_json(serde_json::json!({
        "path": raw_path,
        "deleted": true,
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

/// `tree` — formatted recursive directory listing.
pub async fn op_tree(
    op: &opendal::Operator,
    path: Option<&str>,
    limit: Option<usize>,
) -> Result<AgentToolResult, ToolError> {
    let vpath = OpendalFileStorage::normalize_path(path.unwrap_or("/"));
    let max_entries = limit.unwrap_or(500);

    let mut lister = op
        .lister_with(&vpath)
        .recursive(true)
        .await
        .map_err(|e| e.to_string())?;

    // Collect entries as (depth, path, is_dir).
    let mut entries: Vec<(usize, String, bool)> = Vec::new();
    let prefix = vpath.trim_end_matches('/');
    while let Some(entry) = lister.next().await {
        let entry = entry.map_err(|e| e.to_string())?;
        let p = entry.path().to_string();
        let is_dir = entry.metadata().is_dir();

        // Compute depth relative to the scan root.
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

    // Render tree.
    entries.sort_by(|a, b| a.1.cmp(&b.1));
    let mut out = String::new();
    out.push_str(&vpath);
    out.push('\n');
    for (depth, p, is_dir) in &entries {
        let indent = "  ".repeat(*depth);
        let name = p.rsplit('/').next().unwrap_or(p);
        let suffix = if *is_dir { "/" } else { "" };
        out.push_str(&format!("{indent}{name}{suffix}\n"));
    }

    Ok(AgentToolResult::success_json(serde_json::json!({
        "path": vpath,
        "content": out,
        "entries": entries.len(),
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
    let total_size = op
        .stat(vpath)
        .await
        .map(|m| m.content_length())
        .unwrap_or(0);
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
