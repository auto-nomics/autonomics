//! File-backed memory artifacts and structured model-output helpers.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::Path;

use agentik_sdk::types::messages::{ContentBlock, Message, Role};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct MemoryExtraction {
    #[serde(default)]
    pub raw_memory: String,
    #[serde(default)]
    pub rollout_summary: String,
    #[serde(default)]
    pub rollout_slug: Option<String>,
}

impl MemoryExtraction {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.raw_memory.trim().is_empty()
            && self.rollout_summary.trim().is_empty()
            && self.rollout_slug.is_none()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RawMemoryEntry {
    pub session_id: uuid::Uuid,
    pub raw_memory: String,
    pub rollout_summary: String,
    pub rollout_slug: Option<String>,
    pub generated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct MemoryConsolidation {
    pub memory: String,
    pub summary: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct MemoryArtifacts {
    pub raw_memories: String,
    pub memory: String,
    pub summary: String,
    pub changed: bool,
}

pub fn parse_json_object<T: for<'de> Deserialize<'de>>(text: &str) -> Result<T, String> {
    let trimmed = text.trim();
    let source = strip_json_fence(trimmed).unwrap_or(trimmed);
    let start = source
        .find('{')
        .ok_or_else(|| "model output has no JSON object".to_string())?;
    let end = source
        .rfind('}')
        .ok_or_else(|| "model output has unterminated JSON".to_string())?;
    if end <= start {
        return Err("model output has unterminated JSON".to_string());
    }
    serde_json::from_str(&source[start..=end]).map_err(|e| format!("invalid model JSON: {e}"))
}

fn strip_json_fence(text: &str) -> Option<&str> {
    let body = text.strip_prefix("```")?;
    let body = body
        .strip_prefix("json")
        .or_else(|| body.strip_prefix("JSON"))?;
    body.strip_suffix("```")
}

pub fn hash_source<T: Hash>(value: &T) -> String {
    let mut hasher = DefaultHasher::new();
    value.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

pub fn redact_secrets(input: &str) -> String {
    let mut output = String::with_capacity(input.len());
    for line in input.lines() {
        let lower = line.to_ascii_lowercase();
        if lower.contains("api_key=")
            || lower.contains("apikey=")
            || lower.contains("password=")
            || lower.contains("token=")
            || lower.contains("authorization:")
            || lower.contains("bearer ")
        {
            output.push_str("[REDACTED_SECRET]\n");
        } else {
            output.push_str(line);
            output.push('\n');
        }
    }
    if input.ends_with('\n') {
        output
    } else {
        output.pop().unwrap_or_default();
        output
    }
}

pub fn normalize_slug(slug: Option<&str>, session_id: uuid::Uuid) -> String {
    let fallback = session_id.to_string();
    let mut result = String::new();
    let mut last_was_dash = false;
    for ch in slug.unwrap_or(&fallback).chars() {
        if ch.is_ascii_alphanumeric() {
            result.push(ch.to_ascii_lowercase());
            last_was_dash = false;
        } else if !last_was_dash && !result.is_empty() {
            result.push('-');
            last_was_dash = true;
        }
    }
    let result = result.trim_matches('-').to_string();
    if result.is_empty() { fallback } else { result }
}

pub fn render_transcript(messages: &[Message], max_bytes: usize) -> String {
    let mut output = String::new();
    let mut remaining = max_bytes;
    for message in messages {
        let prefix = match message.role {
            Role::User => "USER",
            Role::Assistant => "ASSISTANT",
        };
        for block in &message.content {
            let rendered = match block {
                ContentBlock::Text { text } => Some(text.clone()),
                ContentBlock::ToolUse { name, input, .. } => {
                    Some(format!("tool call {name}: {input}"))
                }
                ContentBlock::ToolResult {
                    content, is_error, ..
                } => Some(format!(
                    "tool result{}: {}",
                    if is_error.unwrap_or_default() {
                        " (error)"
                    } else {
                        ""
                    },
                    content.as_deref().unwrap_or("")
                )),
                ContentBlock::Thinking { .. } | ContentBlock::Image { .. } => None,
            };
            let Some(mut rendered) = rendered else {
                continue;
            };
            if rendered.len() > 2_000 {
                let cut = rendered
                    .char_indices()
                    .map(|(i, _)| i)
                    .take_while(|i| i <= &2_000)
                    .last()
                    .unwrap_or(0);
                rendered = format!("{}\n[TRUNCATED]", &rendered[..cut]);
            }
            let line = format!("{prefix}: {rendered}\n");
            if line.len() >= remaining {
                let prefix_len = line.len() - remaining;
                let cut = line
                    .char_indices()
                    .map(|(i, _)| i)
                    .take_while(|i| *i <= prefix_len)
                    .last()
                    .unwrap_or(0);
                output.push_str(&line[..cut]);
                output.push_str("\n[ROLLOUT_TRUNCATED]\n");
                return output;
            }
            remaining -= line.len();
            output.push_str(&line);
        }
    }
    output
}

pub fn ensure_memory_layout(root: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(root.join("rollout_summaries"))?;
    std::fs::create_dir_all(root.join("extensions/ad_hoc/notes"))?;
    Ok(())
}

pub fn write_atomic(path: &Path, contents: &str) -> std::io::Result<()> {
    let parent = path.parent().ok_or_else(|| {
        std::io::Error::other(format!("memory path has no parent: {}", path.display()))
    })?;
    std::fs::create_dir_all(parent)?;
    let temp = path.with_extension("tmp");
    std::fs::write(&temp, contents)?;
    std::fs::rename(&temp, path)
}

pub fn normalize_summary(summary: &str) -> String {
    if summary.trim_start().starts_with("v1") {
        summary.trim().to_string()
    } else {
        format!("v1\n\n{}", summary.trim())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_fenced_json_and_redacts_secret_lines() {
        let value: MemoryExtraction = parse_json_object(
            "```json\n{\"raw_memory\":\"token=abc\",\"rollout_summary\":\"s\"}\n```",
        )
        .unwrap();
        assert_eq!(value.rollout_summary, "s");
        assert!(redact_secrets("safe\ntoken=abc").contains("[REDACTED_SECRET]"));
    }

    #[test]
    fn normalizes_slugs_and_summary_schema() {
        assert_eq!(normalize_slug(Some("A b__C"), uuid::Uuid::nil()), "a-b-c");
        assert!(normalize_summary("facts").starts_with("v1\n"));
    }
}
