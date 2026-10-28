use std::sync::Arc;

use agentik_proc::tool;
use agentik_sdk::types::tools::ToolResult;
use async_trait::async_trait;

use crate::memory::{MEMORY_SCOPE_ID, MemoryBackend};
use crate::tools::{ToolError, ToolFunction};

use super::util::store_error;

const DEFAULT_LIST_RESULTS: usize = 100;
const MAX_LIST_RESULTS: usize = 500;

#[tool(
    name = "memory_list",
    description = "List active persistent memory entries ordered by most recent update."
)]
pub struct MemoryListInput {
    #[desc = "Maximum entries returned. Default 100; max 500."]
    pub limit: Option<usize>,
}

pub struct MemoryListTool {
    backend: Arc<MemoryBackend>,
}

impl MemoryListTool {
    pub fn new(backend: Arc<MemoryBackend>) -> Self {
        Self { backend }
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

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use uuid::Uuid;

    use crate::memory::{
        MEMORY_SCOPE_ID, MemoryBackend, MemoryConfig, MemoryEntry, MemoryStore, TursoMemoryStorage,
    };

    use super::*;

    #[tokio::test]
    async fn lists_active_entries() {
        let store: Arc<dyn MemoryStore> =
            Arc::new(TursoMemoryStorage::open_in_memory().await.unwrap());
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

        let result = MemoryListTool::new(backend)
            .run(MemoryListInput { limit: Some(1) })
            .await
            .unwrap();
        assert!(result.text_content().contains("Verification workflow"));
    }
}
