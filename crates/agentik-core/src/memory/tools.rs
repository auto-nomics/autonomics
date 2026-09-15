//! Dedicated database-backed memory tools.

use std::sync::Arc;

use agentik_proc::tool;
use agentik_sdk::types::tools::ToolResult;
use async_trait::async_trait;
use uuid::Uuid;

use super::{MEMORY_SCOPE_ID, MemoryBackend, artifacts::redact_secrets};
use crate::tools::{ToolError, ToolFunction, ToolRegistration};

const DEFAULT_READ_LINES: usize = 200;
const MAX_READ_LINES: usize = 1_000;
const DEFAULT_SEARCH_RESULTS: usize = 20;
const MAX_SEARCH_RESULTS: usize = 100;
const DEFAULT_LIST_RESULTS: usize = 100;
const MAX_LIST_RESULTS: usize = 500;

#[tool(
    name = "memory_search",
    description = "Search persistent agent memory stored in Turso. Queries are case-insensitive \
                   substrings; match_mode=all requires every query in one record."
)]
pub struct MemorySearchInput {
    #[desc = "Substring queries matched by any or all, depending on match_mode."]
    pub queries: Vec<String>,
    #[desc = "Require any (default) or all queries to match a record."]
    pub match_mode: Option<String>,
    #[desc = "Maximum records returned. Default 20; max 100."]
    pub max_results: Option<usize>,
}

#[tool(
    name = "memory_read",
    description = "Read the current memory summary or one active memory entry by UUID."
)]
pub struct MemoryReadInput {
    #[desc = "'summary' for the compact summary, or an entry UUID returned by memory_search."]
    pub target: String,
    #[desc = "Starting 1-indexed line. Default 1."]
    pub offset: Option<usize>,
    #[desc = "Maximum lines returned. Default 200; max 1000."]
    pub limit: Option<usize>,
}

#[tool(
    name = "memory_list",
    description = "List active persistent memory entries ordered by most recent update."
)]
pub struct MemoryListInput {
    #[desc = "Maximum entries returned. Default 100; max 500."]
    pub limit: Option<usize>,
}

#[tool(
    name = "memory_note",
    description = "Queue one small user-requested memory update for the next consolidation run."
)]
pub struct MemoryNoteInput {
    #[desc = "Concise user-requested addition, deletion, or correction."]
    pub content: String,
    #[desc = "Short stable slug for the note."]
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
        if !self.backend.effective_memory_config().use_memory {
            return Ok(ToolResult::error("memory is disabled for this agent"));
        }
        if input.queries.iter().any(String::is_empty) {
            return Ok(ToolResult::error("memory search queries must not be empty"));
        }
        let entries = self
            .backend
            .store
            .list_entries(MEMORY_SCOPE_ID, MAX_SEARCH_RESULTS * 10)
            .await
            .map_err(store_error)?;
        let notes = self
            .backend
            .store
            .list_pending_notes(MEMORY_SCOPE_ID)
            .await
            .map_err(store_error)?;
        let require_all = input.match_mode.as_deref().unwrap_or("any") == "all";
        let max_results = input
            .max_results
            .unwrap_or(DEFAULT_SEARCH_RESULTS)
            .clamp(1, MAX_SEARCH_RESULTS);

        let mut matches = Vec::new();
        for entry in entries {
            let haystack = format!("{}\n{}", entry.title, entry.body_md).to_lowercase();
            if matches_queries(&input.queries, &haystack, require_all) {
                matches.push(serde_json::json!({
                    "kind": "entry",
                    "id": entry.id,
                    "entry_type": entry.entry_type,
                    "title": entry.title,
                    "snippet": snippet(&entry.body_md, &input.queries),
                    "updated_at": entry.updated_at,
                }));
                if matches.len() >= max_results {
                    break;
                }
            }
        }
        if matches.len() < max_results {
            for note in notes {
                let haystack = format!("{}\n{}", note.slug, note.content).to_lowercase();
                if matches_queries(&input.queries, &haystack, require_all) {
                    matches.push(serde_json::json!({
                        "kind": "pending_note",
                        "id": note.id,
                        "title": note.slug,
                        "snippet": snippet(&note.content, &input.queries),
                        "created_at": note.created_at,
                    }));
                    if matches.len() >= max_results {
                        break;
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
        if !self.backend.effective_memory_config().use_memory {
            return Ok(ToolResult::error("memory is disabled for this agent"));
        }
        let offset = input.offset.unwrap_or(1).max(1);
        let limit = input
            .limit
            .unwrap_or(DEFAULT_READ_LINES)
            .clamp(1, MAX_READ_LINES);

        if input.target.eq_ignore_ascii_case("summary") {
            let summary = self
                .backend
                .store
                .get_summary(MEMORY_SCOPE_ID)
                .await
                .map_err(store_error)?;
            let Some(summary) = summary else {
                return Ok(ToolResult::error(
                    "memory summary has not been generated yet",
                ));
            };
            let lines: Vec<&str> = summary
                .summary_md
                .lines()
                .skip(offset - 1)
                .take(limit)
                .collect();
            return Ok(ToolResult::success_json(serde_json::json!({
                "kind": "summary",
                "schema_version": summary.schema_version,
                "offset": offset,
                "lines": lines,
                "returned": lines.len(),
            })));
        }

        let entry_id =
            Uuid::parse_str(input.target.trim()).map_err(|e| ToolError::ValidationFailed {
                message: format!("invalid memory entry UUID: {e}"),
            })?;
        let entry = self
            .backend
            .store
            .get_entry(MEMORY_SCOPE_ID, entry_id)
            .await
            .map_err(store_error)?
            .ok_or_else(|| ToolError::ValidationFailed {
                message: format!("memory entry {entry_id} not found"),
            })?;
        let lines: Vec<&str> = entry.body_md.lines().skip(offset - 1).take(limit).collect();
        Ok(ToolResult::success_json(serde_json::json!({
            "kind": "entry",
            "id": entry.id,
            "entry_type": entry.entry_type,
            "title": entry.title,
            "status": entry.status,
            "confidence": entry.confidence,
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
        if !self.backend.effective_memory_config().use_memory {
            return Ok(ToolResult::error("memory is disabled for this agent"));
        }
        let limit = input
            .limit
            .unwrap_or(DEFAULT_LIST_RESULTS)
            .clamp(1, MAX_LIST_RESULTS);
        let entries = self
            .backend
            .store
            .list_entries(MEMORY_SCOPE_ID, limit)
            .await
            .map_err(store_error)?;
        let values: Vec<serde_json::Value> = entries
            .iter()
            .map(|entry| {
                serde_json::json!({
                    "id": entry.id,
                    "entry_type": entry.entry_type,
                    "title": entry.title,
                    "updated_at": entry.updated_at,
                })
            })
            .collect();
        Ok(ToolResult::success_json(serde_json::json!({
            "entries": values,
            "returned": values.len(),
        })))
    }
}

#[async_trait]
impl ToolFunction for MemoryNoteTool {
    type Input = MemoryNoteInput;

    async fn run(&self, input: Self::Input) -> Result<ToolResult, ToolError> {
        if !self.backend.effective_memory_config().use_memory {
            return Ok(ToolResult::error("memory is disabled for this agent"));
        }
        let content = redact_secrets(input.content.trim());
        if content.is_empty() {
            return Ok(ToolResult::error("memory note content must not be empty"));
        }
        let note = self
            .backend
            .store
            .insert_note(MEMORY_SCOPE_ID, &note_slug(input.slug.as_deref()), &content)
            .await
            .map_err(store_error)?;
        Ok(ToolResult::success_json(serde_json::json!({
            "id": note.id,
            "slug": note.slug,
            "status": "pending",
        })))
    }
}

fn matches_queries(queries: &[String], haystack: &str, require_all: bool) -> bool {
    if require_all {
        queries
            .iter()
            .all(|query| haystack.contains(&query.to_lowercase()))
    } else {
        queries
            .iter()
            .any(|query| haystack.contains(&query.to_lowercase()))
    }
}

fn snippet(content: &str, queries: &[String]) -> String {
    let lower = content.to_lowercase();
    let position = queries
        .iter()
        .filter_map(|query| lower.find(&query.to_lowercase()))
        .min()
        .unwrap_or(0);
    let start = lower
        .char_indices()
        .map(|(i, _)| i)
        .take_while(|i| *i <= position.saturating_sub(120))
        .last()
        .unwrap_or(0);
    let mut result = String::new();
    for ch in content[start..].chars().take(360) {
        result.push(ch);
    }
    if content[start..].chars().count() > 360 {
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

fn store_error(error: crate::storage::StorageError) -> ToolError {
    ToolError::ExecutionFailed {
        source: Box::new(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::{MemoryConfig, MemoryEntry};

    #[tokio::test]
    async fn search_read_list_and_note_use_turso() {
        let store: Arc<dyn crate::memory::MemoryStore> =
            Arc::new(crate::TursoAgentStorage::open_in_memory().await.unwrap());
        let backend = Arc::new(MemoryBackend::new(
            MemoryConfig::new(),
            Arc::clone(&store),
            None,
        ));
        let now = crate::memory::now_ms();
        store
            .complete_phase2(
                MEMORY_SCOPE_ID,
                "source",
                vec![MemoryEntry {
                    id: Uuid::new_v4(),
                    scope_id: MEMORY_SCOPE_ID,
                    entry_type: "workflow".into(),
                    title: "Verification workflow".into(),
                    body_md: "Always run targeted tests before cargo check.".into(),
                    status: "active".into(),
                    confidence: 0.9,
                    created_at: now,
                    updated_at: now,
                }],
                "v1\n\nRun tests.",
                Vec::new(),
                Vec::new(),
            )
            .await
            .unwrap();

        let search = MemorySearchTool {
            backend: Arc::clone(&backend),
        };
        let result = search
            .run(MemorySearchInput {
                queries: vec!["TARGETED".into()],
                match_mode: None,
                max_results: Some(1),
            })
            .await
            .unwrap();
        assert!(result.text_content().contains("Verification workflow"));

        let entries = store.list_entries(MEMORY_SCOPE_ID, 10).await.unwrap();
        let read = MemoryReadTool {
            backend: Arc::clone(&backend),
        };
        let result = read
            .run(MemoryReadInput {
                target: entries[0].id.to_string(),
                offset: None,
                limit: None,
            })
            .await
            .unwrap();
        assert!(result.text_content().contains("targeted tests"));

        let note = MemoryNoteTool {
            backend: Arc::clone(&backend),
        };
        let result = note
            .run(MemoryNoteInput {
                content: "token=abc\nPrefer concise summaries.".into(),
                slug: Some("User Preference!".into()),
            })
            .await
            .unwrap();
        assert!(result.text_content().contains("user-preference"));
        assert!(
            store
                .list_pending_notes(MEMORY_SCOPE_ID)
                .await
                .unwrap()
                .len()
                == 1
        );
    }
}
