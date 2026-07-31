use std::path::Path;

use agentik_sdk::types::ToolImageSource;
use agentik_sdk::types::{ToolResult, ToolResultBlock, ToolResultContent};
use async_trait::async_trait;
use base64::Engine as _;
use tokio::fs;

use agentik_core::tools::{ToolError, ToolFunction};
use agentik_proc::tool;

/// Maximum output size for a text read, matching Claude Code's default cap.
const MAX_SIZE_BYTES: usize = 256 * 1024; // 256 KB
/// Default maximum lines when no explicit limit is provided.
const DEFAULT_MAX_LINES: usize = 2000;
/// Image extensions recognised by this tool.
const IMAGE_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "gif", "webp"];

#[tool(
    name = "read",
    description = "Reads a file from the local filesystem. Supports text files (with line numbers, cat -n style), images (PNG/JPG/JPEG/GIF/WEBP), and Jupyter notebooks (.ipynb). Use offset/limit for partial reads of large text files."
)]
pub struct ReadInput {
    #[desc = "The absolute path of the file to read"]
    pub file_path: String,
    #[desc = "The line number to start reading from (1-indexed). Only provide if the file is too large to read at once."]
    pub offset: Option<usize>,
    #[desc = "The number of lines to read. Only provide if the file is too large to read at once."]
    pub limit: Option<usize>,
}

pub struct ReadTool;

#[async_trait]
impl ToolFunction for ReadTool {
    type Input = ReadInput;

    async fn run(&self, input: Self::Input) -> Result<ToolResult, ToolError> {
        let path = Path::new(&input.file_path);
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_lowercase())
            .unwrap_or_default();

        // ── Notebook (.ipynb) ──
        if ext == "ipynb" {
            return read_notebook(path, &input.file_path).await;
        }

        // ── Image ──
        if IMAGE_EXTENSIONS.contains(&ext.as_str()) {
            return read_image(path, &input.file_path, &ext).await;
        }

        // ── Text (default) ──
        read_text(path, &input.file_path, input.offset, input.limit).await
    }
}

// ─────────────────────────── text ───────────────────────────

async fn read_text(
    path: &Path,
    file_path: &str,
    offset: Option<usize>,
    limit: Option<usize>,
) -> Result<ToolResult, ToolError> {
    // Pre-check file size — avoid reading huge files into memory.
    match fs::metadata(path).await {
        Ok(meta) => {
            let size = meta.len() as usize;
            if size > MAX_SIZE_BYTES && limit.is_none() {
                return Ok(ToolResult::error(format!(
                    "File is too large ({size} bytes > {MAX_SIZE_BYTES} bytes). \
                     Use the offset and limit parameters to read a portion of the file."
                )));
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(ToolResult::error(format!(
                "File does not exist: {file_path}"
            )));
        }
        Err(e) => {
            return Ok(ToolResult::error(format!(
                "Failed to stat {file_path}: {e}"
            )));
        }
    }

    let content = match fs::read_to_string(path).await {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(ToolResult::error(format!(
                "File does not exist: {file_path}"
            )));
        }
        Err(e) => {
            return Ok(ToolResult::error(format!(
                "Failed to read {file_path}: {e}"
            )));
        }
    };

    let lines: Vec<&str> = content.lines().collect();
    let total = lines.len();

    let start = offset.unwrap_or(1).saturating_sub(1).min(total);
    let end = match limit {
        Some(n) => (start + n).min(total),
        None => {
            // Apply DEFAULT_MAX_LINES cap when no explicit limit.
            (start + DEFAULT_MAX_LINES).min(total)
        }
    };

    let width = end.to_string().len().max(3);
    let mut out = String::new();
    for (i, line) in lines[start..end].iter().enumerate() {
        let n = start + i + 1;
        out.push_str(&format!("{n:>width$}\t{line}\n"));
    }

    if out.is_empty() {
        if total == 0 {
            out.push_str("(file exists but is empty)");
        } else if start >= total {
            out.push_str(&format!(
                "(offset {start} exceeds file length {total}; file has {total} lines)"
            ));
        }
    }

    // Truncation notice when we hit the DEFAULT_MAX_LINES cap.
    if limit.is_none() && end - start == DEFAULT_MAX_LINES && end < total {
        let remaining = total - end;
        out.push_str(&format!(
            "\n... ({remaining} more lines, use offset={} to continue)\n",
            end + 1
        ));
    }

    Ok(ToolResult::success(out))
}

// ─────────────────────────── image ───────────────────────────

async fn read_image(path: &Path, file_path: &str, ext: &str) -> Result<ToolResult, ToolError> {
    let bytes = match fs::read(path).await {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(ToolResult::error(format!(
                "File does not exist: {file_path}"
            )));
        }
        Err(e) => {
            return Ok(ToolResult::error(format!(
                "Failed to read image {file_path}: {e}"
            )));
        }
    };

    if bytes.is_empty() {
        return Ok(ToolResult::error(format!(
            "Image file is empty: {file_path}"
        )));
    }

    let media_type = image_media_type(ext);
    let data = base64::engine::general_purpose::STANDARD.encode(&bytes);

    Ok(ToolResult {
        tool_use_id: String::new(),
        content: ToolResultContent::Blocks(vec![ToolResultBlock::Image {
            source: ToolImageSource::Base64 {
                media_type: media_type.to_string(),
                data,
            },
        }]),
        is_error: Some(false),
    })
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

async fn read_notebook(path: &Path, file_path: &str) -> Result<ToolResult, ToolError> {
    let raw = match fs::read_to_string(path).await {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(ToolResult::error(format!(
                "File does not exist: {file_path}"
            )));
        }
        Err(e) => {
            return Ok(ToolResult::error(format!(
                "Failed to read notebook {file_path}: {e}"
            )));
        }
    };

    let nb: serde_json::Value = match serde_json::from_str(&raw) {
        Ok(v) => v,
        Err(e) => {
            return Ok(ToolResult::error(format!(
                "Invalid notebook JSON in {file_path}: {e}"
            )));
        }
    };

    let cells = nb.get("cells").and_then(|c| c.as_array());
    let cell_count = cells.map_or(0, |c| c.len());

    let mut out = String::new();
    out.push_str(&format!(
        "Jupyter notebook: {file_path}\n{cell_count} cells\n\n"
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

    Ok(ToolResult::success(out))
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

#[cfg(test)]
mod tests {
    use super::*;

    // ── text ──

    #[tokio::test]
    async fn test_read_full() {
        let dir = std::env::temp_dir();
        let path = dir.join("agentik_read_test.txt");
        std::fs::write(&path, "a\nb\nc\n").unwrap();
        let tool = ReadTool;
        let result = tool
            .run(ReadInput {
                file_path: path.display().to_string(),
                offset: None,
                limit: None,
            })
            .await
            .unwrap();
        match &result.content {
            ToolResultContent::Text(t) => {
                assert!(t.contains("1\ta"));
                assert!(t.contains("3\tc"));
            }
            other => panic!("expected text, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_read_offset_limit() {
        let dir = std::env::temp_dir();
        let path = dir.join("agentik_read_range_test.txt");
        std::fs::write(&path, "1\n2\n3\n4\n5\n").unwrap();
        let tool = ReadTool;
        let result = tool
            .run(ReadInput {
                file_path: path.display().to_string(),
                offset: Some(2),
                limit: Some(2),
            })
            .await
            .unwrap();
        match &result.content {
            ToolResultContent::Text(t) => {
                assert!(t.contains("2\t2"));
                assert!(t.contains("3\t3"));
                assert!(!t.contains("4\t4"));
            }
            other => panic!("expected text, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_read_missing_file() {
        let tool = ReadTool;
        let result = tool
            .run(ReadInput {
                file_path: "/nonexistent/agentik/nope.txt".to_string(),
                offset: None,
                limit: None,
            })
            .await
            .unwrap();
        assert_eq!(result.is_error, Some(true));
    }

    #[tokio::test]
    async fn test_read_empty_file() {
        let dir = std::env::temp_dir();
        let path = dir.join("agentik_read_empty.txt");
        std::fs::write(&path, "").unwrap();
        let tool = ReadTool;
        let result = tool
            .run(ReadInput {
                file_path: path.display().to_string(),
                offset: None,
                limit: None,
            })
            .await
            .unwrap();
        match &result.content {
            ToolResultContent::Text(t) => assert!(t.contains("empty")),
            other => panic!("expected text, got {other:?}"),
        }
    }

    // ── image ──

    #[tokio::test]
    async fn test_read_image() {
        let dir = std::env::temp_dir();
        let path = dir.join("agentik_read_test.png");
        // Minimal 1×1 PNG.
        let png_bytes: &[u8] = &[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
        std::fs::write(&path, png_bytes).unwrap();
        let tool = ReadTool;
        let result = tool
            .run(ReadInput {
                file_path: path.display().to_string(),
                offset: None,
                limit: None,
            })
            .await
            .unwrap();
        match &result.content {
            ToolResultContent::Blocks(blocks) => {
                assert_eq!(blocks.len(), 1);
                match &blocks[0] {
                    ToolResultBlock::Image { source } => match source {
                        ToolImageSource::Base64 { media_type, data } => {
                            assert_eq!(media_type, "image/png");
                            assert!(!data.is_empty());
                        }
                    },
                    other => panic!("expected image block, got {other:?}"),
                }
            }
            other => panic!("expected blocks, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_read_image_missing() {
        let tool = ReadTool;
        let result = tool
            .run(ReadInput {
                file_path: "/nonexistent/img.png".to_string(),
                offset: None,
                limit: None,
            })
            .await
            .unwrap();
        assert_eq!(result.is_error, Some(true));
    }

    // ── notebook ──

    #[tokio::test]
    async fn test_read_notebook() {
        let dir = std::env::temp_dir();
        let path = dir.join("agentik_read_test.ipynb");
        let nb = serde_json::json!({
            "cells": [
                {
                    "cell_type": "markdown",
                    "source": ["# Title\n", "Some text"]
                },
                {
                    "cell_type": "code",
                    "source": "print(1+1)\n",
                    "outputs": [
                        {"text": "2\n"}
                    ]
                }
            ],
            "metadata": {},
            "nbformat": 4,
            "nbformat_minor": 5
        });
        std::fs::write(&path, serde_json::to_string(&nb).unwrap()).unwrap();

        let tool = ReadTool;
        let result = tool
            .run(ReadInput {
                file_path: path.display().to_string(),
                offset: None,
                limit: None,
            })
            .await
            .unwrap();
        match &result.content {
            ToolResultContent::Text(t) => {
                assert!(t.contains("2 cells"));
                assert!(t.contains("[markdown]"));
                assert!(t.contains("# Title"));
                assert!(t.contains("[code]"));
                assert!(t.contains("print(1+1)"));
                assert!(t.contains("[output]"));
                assert!(t.contains("2"));
            }
            other => panic!("expected text, got {other:?}"),
        }
    }
}
