use std::sync::Arc;

use agentik_core::tools::{ToolError, ToolFunction};
use agentik_sdk::types::{
    ToolImageSource, ToolResult as AgentToolResult, ToolResultBlock,
};
use async_trait::async_trait;
use base64::Engine as _;

use agentik_proc::tool;

use crate::storage::OpendalFileStorage;

/// Default maximum lines returned when no explicit `limit` is given.
/// Matches Claude Code's FileReadTool default.
const DEFAULT_MAX_LINES: usize = 2000;
/// Image extensions recognised by this tool.
const IMAGE_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "gif", "webp"];

#[tool(
    name = "file_read",
    description = "Reads a file from the virtual filesystem. \
        Supports text files (returned with line numbers, cat -n style), \
        images (PNG/JPG/JPEG/GIF/WEBP, returned as base64 image blocks), \
        and Jupyter notebooks (.ipynb, returned as formatted text with cells and outputs). \
        Use offset/limit for partial reads of large text files."
)]
pub struct FileReadInput {
    #[desc = "Path to the file to read."]
    pub path: String,
    #[desc = "Line number to start reading from (1-indexed). Defaults to 1."]
    pub offset: Option<usize>,
    #[desc = "Maximum number of lines to read. Defaults to 2000 if not set."]
    pub limit: Option<usize>,
}

pub struct FileReadTool {
    pub(crate) storage: Arc<OpendalFileStorage>,
}

#[async_trait]
impl ToolFunction for FileReadTool {
    type Input = FileReadInput;

    async fn run(&self, input: Self::Input) -> Result<AgentToolResult, ToolError> {
        let op = &self.storage.op;
        let path = OpendalFileStorage::normalize_path(&input.path);

        // Detect format from file extension.
        let ext = std::path::Path::new(&input.path)
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_lowercase())
            .unwrap_or_default();

        // ── Notebook (.ipynb) ──
        if ext == "ipynb" {
            return read_notebook(op, &path, &input.path).await;
        }

        // ── Image ──
        if IMAGE_EXTENSIONS.contains(&ext.as_str()) {
            return read_image(op, &path, &input.path, &ext).await;
        }

        // ── Text (default) ──
        read_text(op, &path, &input.path, input.offset, input.limit).await
    }
}

// ─────────────────────────── text ───────────────────────────

async fn read_text(
    op: &opendal::Operator,
    path: &str,
    display_path: &str,
    offset: Option<usize>,
    limit: Option<usize>,
) -> Result<AgentToolResult, ToolError> {
    // Stat for total size reporting.
    let total_size = op
        .stat(path)
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

    // Read the full file — we need line-based slicing which requires the
    // complete text. OpenDAL doesn't provide line-aware range reads, and
    // the files this tool targets are source/config files well within
    // memory budget.
    let reader = op.reader(path).await.map_err(|e| e.to_string())?;
    let buf = reader
        .read(0..total_size)
        .await
        .map_err(|e| e.to_string())?;

    // Use lossy conversion so non-UTF-8 files don't crash the tool.
    let bytes = buf.to_vec();
    let content = String::from_utf8_lossy(&bytes);

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

    if out.is_empty() {
        if start >= total_lines {
            out.push_str(&format!(
                "(offset {} exceeds file length; file has {total_lines} lines)"
            , offset.unwrap_or(1)));
        }
    }

    // Truncation notice when we hit the DEFAULT_MAX_LINES cap.
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

// ─────────────────────────── image ───────────────────────────

async fn read_image(
    op: &opendal::Operator,
    path: &str,
    display_path: &str,
    ext: &str,
) -> Result<AgentToolResult, ToolError> {
    let total_size = op
        .stat(path)
        .await
        .map(|m| m.content_length())
        .unwrap_or(0);

    if total_size == 0 {
        return Ok(AgentToolResult::error(format!(
            "Image file is empty: {display_path}"
        )));
    }

    let reader = op.reader(path).await.map_err(|e| e.to_string())?;
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

/// Maps a file extension to its MIME type.
fn image_media_type(ext: &str) -> &'static str {
    match ext {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        _ => "application/octet-stream",
    }
}

// ──────────────────────── notebook (.ipynb) ────────────────────────

async fn read_notebook(
    op: &opendal::Operator,
    path: &str,
    display_path: &str,
) -> Result<AgentToolResult, ToolError> {
    let total_size = op
        .stat(path)
        .await
        .map(|m| m.content_length())
        .unwrap_or(0);

    if total_size == 0 {
        return Ok(AgentToolResult::error(format!(
            "Notebook file is empty: {display_path}"
        )));
    }

    let reader = op.reader(path).await.map_err(|e| e.to_string())?;
    let buf = reader
        .read(0..total_size)
        .await
        .map_err(|e| e.to_string())?;

    let nb_bytes = buf.to_vec();
    let raw = String::from_utf8_lossy(&nb_bytes);

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

            // Show outputs for code cells.
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
