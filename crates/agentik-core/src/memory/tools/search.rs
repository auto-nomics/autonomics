use std::sync::Arc;

use agentik_proc::tool;
use agentik_sdk::types::tools::ToolResult;
use async_trait::async_trait;

use crate::memory::{KmsAgentHandle, MEMORY_SCOPE_ID};
use crate::tools::{ToolError, ToolFunction};

use super::util::store_error;

const DEFAULT_SEARCH_RESULTS: usize = 20;
const MAX_SEARCH_RESULTS: usize = 100;

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

pub struct MemorySearchTool {
    backend: Arc<KmsAgentHandle>,
}

impl MemorySearchTool {
    pub fn new(backend: Arc<KmsAgentHandle>) -> Self {
        Self { backend }
    }
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
        let nodes = self
            .backend
            .store()
            .list_knowledge(MAX_SEARCH_RESULTS * 10)
            .await
            .map_err(store_error)?;
        let notes = self
            .backend
            .store()
            .list_pending_notes(MEMORY_SCOPE_ID)
            .await
            .map_err(store_error)?;
        let require_all = input.match_mode.as_deref().unwrap_or("any") == "all";
        let max_results = input
            .max_results
            .unwrap_or(DEFAULT_SEARCH_RESULTS)
            .clamp(1, MAX_SEARCH_RESULTS);

        let mut matches = Vec::new();
        for node in nodes {
            let haystack = format!(
                "{}\n{}\n{}",
                node.knowledge_path.as_str(),
                node.title,
                node.body_md
            )
            .to_lowercase();
            if matches_queries(&input.queries, &haystack, require_all) {
                matches.push(serde_json::json!({
                    "kind": "knowledge_node",
                    "id": node.id,
                    "knowledge_path": node.knowledge_path.as_str(),
                    "title": node.title,
                    "structural": node.structural,
                    "snippet": snippet(&node.body_md, &input.queries),
                    "updated_at": node.updated_at,
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

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use uuid::Uuid;

    use crate::memory::{
        KmsAgentHandle, Knowledge, KnowledgePath, MEMORY_SCOPE_ID, MemoryConfig, MemoryStore,
        TursoKmsStorage,
    };

    use super::*;

    #[tokio::test]
    async fn search_entries_and_pending_notes() {
        let store = Arc::new(TursoKmsStorage::open_in_memory().await.unwrap());
        let backend = Arc::new(KmsAgentHandle::new(MemoryConfig::new(), Arc::clone(&store)));
        let now = crate::memory::now_ms();
        store
            .complete_phase2(
                MEMORY_SCOPE_ID,
                "source",
                vec![Knowledge::leaf(
                    KnowledgePath::new(["engineering", "testing", "verification"]),
                    "Verification workflow".into(),
                    "Always run targeted tests before cargo check.".into(),
                    0.9,
                    "source",
                    now,
                )],
                "v1\n\nRun tests.",
                Vec::new(),
                Vec::new(),
            )
            .await
            .unwrap();
        store
            .insert_note(
                MEMORY_SCOPE_ID,
                "pending-note",
                "Remember release checklist",
            )
            .await
            .unwrap();

        let result = MemorySearchTool::new(Arc::clone(&backend))
            .run(MemorySearchInput {
                queries: vec!["TARGETED".into()],
                match_mode: None,
                max_results: Some(1),
            })
            .await
            .unwrap();
        assert!(result.text_content().contains("Verification workflow"));

        let result = MemorySearchTool::new(backend)
            .run(MemorySearchInput {
                queries: vec!["checklist".into()],
                match_mode: None,
                max_results: Some(1),
            })
            .await
            .unwrap();
        assert!(result.text_content().contains("pending_note"));
    }
}
