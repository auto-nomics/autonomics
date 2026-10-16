//! Reading operations: `read`, `cat`, `head`, `tail`.

use agentik_core::tools::{ToolError, ToolResult};
use agentik_sdk::types::{ToolImageSource, ToolResult as AgentToolResult, ToolResultBlock};
use base64::Engine as _;

use super::common::{binary_refused, is_binary, require_path};
use crate::storage::OpendalFileStorage;

/// Default maximum lines for `read` when no explicit `limit` is given.
const DEFAULT_MAX_LINES: usize = 2000;
/// Maximum file size that `cat` will read into memory. Callers may request a
/// lower per-call limit, but cannot exceed this hard cap.
const CAT_MAX_BYTES: usize = 10 * 1024 * 1024;
/// Image extensions recognised by the `read` op.
const IMAGE_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "gif", "webp"];

/// `read` — rich read with line numbers (text), base64 (image), or formatted (notebook).
pub async fn op_read(
    storage: &OpendalFileStorage,
    path: Option<&str>,
    offset: Option<usize>,
    limit: Option<usize>,
) -> Result<AgentToolResult, ToolError> {
    let raw_path = require_path(path, "read")?;
    let vpath = OpendalFileStorage::normalize_path(raw_path);
    storage
        .check_readable(&vpath)
        .map_err(|error| error.to_string())?;
    let op = storage.resolve(&vpath);
    let remote = storage.resolve_path(&vpath);

    let ext = std::path::Path::new(raw_path)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase())
        .unwrap_or_default();

    if ext == "ipynb" {
        return read_notebook(&op, &remote, raw_path).await;
    }
    if IMAGE_EXTENSIONS.contains(&ext.as_str()) {
        return read_image(&op, &remote, raw_path, &ext).await;
    }
    read_text_numbered(&op, &remote, raw_path, offset, limit).await
}

/// `cat` — plain text read, no line numbers, no special formatting.
pub async fn op_cat(
    storage: &OpendalFileStorage,
    path: Option<&str>,
    offset: Option<usize>,
    limit: Option<usize>,
    max_bytes: Option<usize>,
) -> Result<AgentToolResult, ToolError> {
    let raw_path = require_path(path, "cat")?;
    let max_bytes = max_bytes.unwrap_or(CAT_MAX_BYTES);
    if max_bytes == 0 || max_bytes > CAT_MAX_BYTES {
        return Err(ToolError::ValidationFailed {
            message: format!("cat 'max_bytes' must be between 1 and {CAT_MAX_BYTES}"),
        });
    }

    let vpath = OpendalFileStorage::normalize_path(raw_path);
    if let Err(error) = storage.check_readable(&vpath) {
        return Ok(AgentToolResult::error(format!(
            "cat: '{raw_path}': {error}"
        )));
    }
    let op = storage.resolve(&vpath);
    let remote = storage.resolve_path(&vpath);

    let meta = match op.stat(&remote).await {
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
    if total_size > max_bytes as u64 {
        return Ok(AgentToolResult::error(format!(
            "cat: '{raw_path}' is too large: {total_size} bytes (max_bytes: {max_bytes})"
        )));
    }

    let buf = op.read(&remote).await.map_err(|e| e.to_string())?;
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
    storage: &OpendalFileStorage,
    path: Option<&str>,
    offset: Option<usize>,
    limit: Option<usize>,
) -> Result<AgentToolResult, ToolError> {
    let raw_path = require_path(path, "head")?;
    let vpath = OpendalFileStorage::normalize_path(raw_path);
    let n = limit.unwrap_or(10);
    let start = offset.unwrap_or(1).saturating_sub(1);
    if let Err(error) = storage.check_readable(&vpath) {
        return Ok(AgentToolResult::error(format!(
            "head: '{raw_path}': {error}"
        )));
    }
    let op = storage.resolve(&vpath);
    let remote = storage.resolve_path(&vpath);

    let meta = match op.stat(&remote).await {
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

    let buf = op.read(&remote).await.map_err(|e| e.to_string())?;
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
    storage: &OpendalFileStorage,
    path: Option<&str>,
    offset: Option<usize>,
    limit: Option<usize>,
) -> Result<AgentToolResult, ToolError> {
    let raw_path = require_path(path, "tail")?;
    let vpath = OpendalFileStorage::normalize_path(raw_path);
    let n = limit.unwrap_or(10);
    if let Err(error) = storage.check_readable(&vpath) {
        return Ok(AgentToolResult::error(format!(
            "tail: '{raw_path}': {error}"
        )));
    }
    let op = storage.resolve(&vpath);
    let remote = storage.resolve_path(&vpath);

    let meta = match op.stat(&remote).await {
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

    let buf = op.read(&remote).await.map_err(|e| e.to_string())?;
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

/// Text read with line numbers (cat -n style).
async fn read_text_numbered(
    op: &opendal::Operator,
    remote_path: &str,
    display_path: &str,
    offset: Option<usize>,
    limit: Option<usize>,
) -> Result<AgentToolResult, ToolError> {
    let meta = match op.stat(remote_path).await {
        Ok(m) => m,
        Err(e) if matches!(e.kind(), opendal::ErrorKind::NotFound) => {
            return Ok(AgentToolResult::error(format!(
                "read: '{display_path}': No such file or directory"
            )));
        }
        Err(e) => {
            return Ok(AgentToolResult::error(format!(
                "read: '{display_path}': {e}"
            )));
        }
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

    let reader = op.reader(remote_path).await.map_err(|e| e.to_string())?;
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
    remote_path: &str,
    display_path: &str,
    ext: &str,
) -> Result<AgentToolResult, ToolError> {
    let total_size = op
        .stat(remote_path)
        .await
        .map(|m| m.content_length())
        .unwrap_or(0);
    if total_size == 0 {
        return Ok(AgentToolResult::error(format!(
            "Image file is empty: {display_path}"
        )));
    }

    let reader = op.reader(remote_path).await.map_err(|e| e.to_string())?;
    let buf = reader
        .read(0..total_size)
        .await
        .map_err(|e| e.to_string())?;

    let media_type = image_media_type(ext);
    let data = base64::engine::general_purpose::STANDARD.encode(buf.to_vec());

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
    remote_path: &str,
    display_path: &str,
) -> Result<AgentToolResult, ToolError> {
    let total_size = op
        .stat(remote_path)
        .await
        .map(|m| m.content_length())
        .unwrap_or(0);
    if total_size == 0 {
        return Ok(AgentToolResult::error(format!(
            "Notebook file is empty: {display_path}"
        )));
    }

    let reader = op.reader(remote_path).await.map_err(|e| e.to_string())?;
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
