//! Dedicated memory read/search tools.
//!
//! Memory intentionally does not go through the general VFS: it may live in
//! the runtime state directory rather than the agent's writable data root,
//! and access is constrained to this memory tree.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use agentik_proc::tool;
use agentik_sdk::types::tools::ToolResult;
use async_trait::async_trait;

use super::backend::{MemoryBackend, safe_memory_path};
use crate::tools::{ToolError, ToolFunction, ToolRegistration};

const DEFAULT_READ_LINES: usize = 200;
const MAX_READ_LINES: usize = 1_000;
const DEFAULT_SEARCH_RESULTS: usize = 20;
const MAX_SEARCH_RESULTS: usize = 100;
const DEFAULT_LIST_RESULTS: usize = 100;
const MAX_LIST_RESULTS: usize = 500;

#[tool(
    name = "memory_search",
    description = "Search persistent agent memory files. Queries are case-insensitive \
                   substrings. Set match_mode to all to require every query on a line. \
                   Paths are relative to the memory root."
)]
pub struct MemorySearchInput {
    #[desc = "Substring queries matched by any or all, depending on match_mode."]
    pub queries: Vec<String>,
    #[desc = "Optional relative file or directory to search."]
    pub path: Option<String>,
    #[desc = "Require any (default) or all queries to match a line."]
    pub match_mode: Option<String>,
    #[desc = "Maximum matching lines returned. Default 20; max 100."]
    pub max_results: Option<usize>,
}

#[tool(
    name = "memory_read",
    description = "Read a persistent memory file by relative path with 1-indexed paging."
)]
pub struct MemoryReadInput {
    #[desc = "Relative path under the memory root, for example MEMORY.md."]
    pub path: String,
    #[desc = "Starting 1-indexed line. Default 1."]
    pub offset: Option<usize>,
    #[desc = "Maximum lines returned. Default 200; max 1000."]
    pub limit: Option<usize>,
}

#[tool(
    name = "memory_list",
    description = "List files in persistent agent memory, optionally recursively."
)]
pub struct MemoryListInput {
    #[desc = "Optional relative directory. Default is the memory root."]
    pub path: Option<String>,
    #[desc = "List recursively. Default false."]
    pub recursive: Option<bool>,
    #[desc = "Maximum entries returned. Default 100; max 500."]
    pub limit: Option<usize>,
}

#[tool(
    name = "memory_note",
    description = "Create one small ad-hoc memory update note when the user explicitly asks \
                   to remember, forget, or change persistent memory. Direct memory artifact \
                   editing is not allowed."
)]
pub struct MemoryNoteInput {
    #[desc = "Concise user-requested addition, deletion, or correction."]
    pub content: String,
    #[desc = "Short filename slug used for the note."]
    pub slug: Option<String>,
}

pub struct MemorySearchTool {
    backend: Arc<MemoryBackend>,
}

pub struct MemoryReadTool {
    backend: Arc<MemoryBackend>,
}

pub struct MemoryListTool {
    backend: Arc<MemoryBackend>,
}

pub struct MemoryNoteTool {
    backend: Arc<MemoryBackend>,
}

#[must_use]
pub fn memory_registrations(backend: Arc<MemoryBackend>) -> Vec<ToolRegistration> {
    vec![
        ToolRegistration::from(MemorySearchTool {
            backend: Arc::clone(&backend),
        }),
        ToolRegistration::from(MemoryReadTool {
            backend: Arc::clone(&backend),
        }),
        ToolRegistration::from(MemoryListTool {
            backend: Arc::clone(&backend),
        }),
        ToolRegistration::from(MemoryNoteTool { backend }),
    ]
}

#[async_trait]
impl ToolFunction for MemorySearchTool {
    type Input = MemorySearchInput;

    async fn run(&self, input: Self::Input) -> Result<ToolResult, ToolError> {
        if input.queries.iter().any(String::is_empty) {
            return Ok(ToolResult::error("memory search queries must not be empty"));
        }
        let root = self.backend.root();
        let scope = match &input.path {
            Some(path) => safe_memory_path(root, path).map_err(validation_error)?,
            None => root.to_path_buf(),
        };
        if !scope.exists() {
            return Ok(ToolResult::error(format!(
                "memory path does not exist: {}",
                input.path.unwrap_or_default()
            )));
        }
        let mut files = Vec::new();
        collect_markdown_files(&scope, &mut files).map_err(tool_error)?;
        files.sort();

        let require_all = input.match_mode.as_deref().unwrap_or("any") == "all";
        let max_results = input
            .max_results
            .unwrap_or(DEFAULT_SEARCH_RESULTS)
            .clamp(1, MAX_SEARCH_RESULTS);
        let mut matches = Vec::new();
        'files: for path in files {
            let Ok(contents) = tokio::fs::read_to_string(&path).await else {
                continue;
            };
            let relative = relative_name(root, &path);
            for (index, line) in contents.lines().enumerate() {
                let haystack = line.to_ascii_lowercase();
                let matched = if require_all {
                    !input.queries.is_empty()
                        && input
                            .queries
                            .iter()
                            .all(|query| haystack.contains(&query.to_ascii_lowercase()))
                } else {
                    input
                        .queries
                        .iter()
                        .any(|query| haystack.contains(&query.to_ascii_lowercase()))
                };
                if matched {
                    matches.push(serde_json::json!({
                        "path": relative,
                        "line": index + 1,
                        "text": truncate_line(line),
                    }));
                    if matches.len() >= max_results {
                        break 'files;
                    }
                }
            }
        }

        Ok(ToolResult::success_json(serde_json::json!({
            "matches": matches,
            "returned": matches.len(),
            "truncated": matches.len() == max_results,
        })))
    }
}

#[async_trait]
impl ToolFunction for MemoryReadTool {
    type Input = MemoryReadInput;

    async fn run(&self, input: Self::Input) -> Result<ToolResult, ToolError> {
        let path = safe_memory_path(self.backend.root(), &input.path).map_err(validation_error)?;
        let contents =
            tokio::fs::read_to_string(&path)
                .await
                .map_err(|e| ToolError::ExecutionFailed {
                    source: Box::new(std::io::Error::other(format!(
                        "read memory file {}: {e}",
                        input.path
                    ))),
                })?;
        let offset = input.offset.unwrap_or(1).max(1);
        let limit = input
            .limit
            .unwrap_or(DEFAULT_READ_LINES)
            .clamp(1, MAX_READ_LINES);
        let lines: Vec<&str> = contents.lines().skip(offset - 1).take(limit).collect();
        Ok(ToolResult::success_json(serde_json::json!({
            "path": input.path,
            "offset": offset,
            "lines": lines,
            "returned": lines.len(),
        })))
    }
}

#[async_trait]
impl ToolFunction for MemoryListTool {
    type Input = MemoryListInput;

    async fn run(&self, input: Self::Input) -> Result<ToolResult, ToolError> {
        let root = self.backend.root();
        let scope = match &input.path {
            Some(path) => safe_memory_path(root, path).map_err(validation_error)?,
            None => root.to_path_buf(),
        };
        if !scope.is_dir() {
            return Ok(ToolResult::error("memory list path is not a directory"));
        }
        let recursive = input.recursive.unwrap_or(false);
        let mut paths = Vec::new();
        if recursive {
            collect_all_files(&scope, &mut paths).map_err(tool_error)?;
        } else {
            let mut read_dir = tokio::fs::read_dir(&scope).await.map_err(tool_error)?;
            while let Some(entry) = read_dir.next_entry().await.map_err(tool_error)? {
                paths.push(entry.path());
            }
        }
        paths.sort();
        let limit = input
            .limit
            .unwrap_or(DEFAULT_LIST_RESULTS)
            .clamp(1, MAX_LIST_RESULTS);
        let entries: Vec<serde_json::Value> = paths
            .into_iter()
            .take(limit)
            .map(|path| {
                serde_json::json!({
                    "path": relative_name(root, &path),
                    "directory": path.is_dir(),
                })
            })
            .collect();
        Ok(ToolResult::success_json(serde_json::json!({
            "entries": entries,
            "returned": entries.len(),
        })))
    }
}

#[async_trait]
impl ToolFunction for MemoryNoteTool {
    type Input = MemoryNoteInput;

    async fn run(&self, input: Self::Input) -> Result<ToolResult, ToolError> {
        let content = input.content.trim();
        if content.is_empty() {
            return Ok(ToolResult::error("memory note content must not be empty"));
        }
        let timestamp = chrono::Utc::now().format("%Y%m%dT%H%M%SZ");
        let slug = note_slug(input.slug.as_deref());
        let relative = format!("extensions/ad_hoc/notes/{timestamp}-{slug}.md");
        let path = safe_memory_path(self.backend.root(), &relative).map_err(validation_error)?;
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(tool_error)?;
        }
        let note = format!(
            "# Ad-hoc memory update\n\nsource: user-requested\n\n{}\n",
            crate::memory::artifacts::redact_secrets(content)
        );
        tokio::fs::write(&path, note).await.map_err(tool_error)?;
        Ok(ToolResult::success_json(serde_json::json!({
            "path": relative,
            "status": "queued_for_next_consolidation",
        })))
    }
}

fn collect_markdown_files(path: &Path, output: &mut Vec<PathBuf>) -> std::io::Result<()> {
    if path.is_file() {
        if path
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("md"))
        {
            output.push(path.to_path_buf());
        }
        return Ok(());
    }
    for entry in std::fs::read_dir(path)? {
        let entry = entry?;
        let child = entry.path();
        if child.is_dir() {
            collect_markdown_files(&child, output)?;
        } else if child
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("md"))
        {
            output.push(child);
        }
    }
    Ok(())
}

fn collect_all_files(path: &Path, output: &mut Vec<PathBuf>) -> std::io::Result<()> {
    for entry in std::fs::read_dir(path)? {
        let entry = entry?;
        let child = entry.path();
        if child.is_dir() {
            output.push(child.clone());
            collect_all_files(&child, output)?;
        } else {
            output.push(child);
        }
    }
    Ok(())
}

fn relative_name(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

fn truncate_line(line: &str) -> String {
    let mut result = String::new();
    for ch in line.chars().take(500) {
        result.push(ch);
    }
    if line.chars().count() > 500 {
        result.push_str("...");
    }
    result
}

fn note_slug(raw: Option<&str>) -> String {
    let fallback = "update";
    let mut slug = String::new();
    for ch in raw.unwrap_or(fallback).chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch.to_ascii_lowercase());
        } else if !slug.ends_with('-') && !slug.is_empty() {
            slug.push('-');
        }
    }
    let slug = slug.trim_matches('-');
    if slug.is_empty() {
        fallback.to_string()
    } else {
        slug.to_string()
    }
}

fn validation_error(message: String) -> ToolError {
    ToolError::ValidationFailed { message }
}

fn tool_error(error: std::io::Error) -> ToolError {
    ToolError::ExecutionFailed {
        source: Box::new(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::MemoryConfig;

    fn unique_temp_dir(label: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "agentik-memory-{label}-{}-{}",
            std::process::id(),
            crate::memory::now_ms()
        ));
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    #[tokio::test]
    async fn search_read_and_note_are_bounded_to_memory_root() {
        let root = unique_temp_dir("tools");
        let backend = Arc::new(MemoryBackend::new(MemoryConfig::new(root.clone())));
        std::fs::write(root.join("MEMORY.md"), "alpha\nbeta\n").unwrap();

        let search = MemorySearchTool {
            backend: Arc::clone(&backend),
        };
        let result = search
            .run(MemorySearchInput {
                queries: vec!["BETA".into()],
                path: None,
                match_mode: None,
                max_results: Some(1),
            })
            .await
            .unwrap();
        assert!(result.text_content().contains("\"line\":2"));

        let read = MemoryReadTool {
            backend: Arc::clone(&backend),
        };
        let result = read
            .run(MemoryReadInput {
                path: "../escape".into(),
                offset: None,
                limit: None,
            })
            .await;
        assert!(result.is_err());

        let note = MemoryNoteTool {
            backend: backend.clone(),
        };
        let result = note
            .run(MemoryNoteInput {
                content: "token=abc\nremember preferences".into(),
                slug: Some("User Preference!".into()),
            })
            .await
            .unwrap();
        assert!(result.text_content().contains("user-preference"));
        std::fs::remove_dir_all(root).ok();
    }
}
