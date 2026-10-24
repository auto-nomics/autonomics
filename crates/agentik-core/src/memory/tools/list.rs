use std::sync::Arc;

use agentik_proc::tool;
use agentik_sdk::types::tools::ToolResult;
use async_trait::async_trait;

use crate::memory::{KmsAgentHandle, MEMORY_SCOPE_ID};
use crate::tools::{ToolError, ToolFunction};

use super::util::store_error;

const DEFAULT_LIST_RESULTS: usize = 100;
const MAX_LIST_RESULTS: usize = 500;

#[tool(
    name = "memory_list",
    description = "List active knowledge nodes ordered by most recent update."
)]
pub struct MemoryListInput {
    #[desc = "Maximum nodes returned. Default 100; max 500."]
    pub limit: Option<usize>,
}

pub struct MemoryListTool {
    backend: Arc<KmsAgentHandle>,
}

impl MemoryListTool {
    pub fn new(backend: Arc<KmsAgentHandle>) -> Self {
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
        let stored_nodes = self
            .backend
            .store()
            .list_knowledge(MAX_LIST_RESULTS)
            .await
            .map_err(store_error)?;
        let values: Vec<serde_json::Value> = stored_nodes
            .iter()
            .filter(|node| !node.structural)
            .take(limit)
            .map(|node| {
                serde_json::json!({
                    "id": node.id,
                    "knowledge_path": node.knowledge_path.as_str(),
                    "title": node.title,
                    "structural": node.structural,
                    "updated_at": node.updated_at,
                })
            })
            .collect();
        Ok(ToolResult::success_json(serde_json::json!({
            "nodes": values,
            "returned": values.len(),
        })))
    }
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
    async fn lists_active_entries() {
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

        let result = MemoryListTool::new(backend)
            .run(MemoryListInput { limit: Some(1) })
            .await
            .unwrap();
        assert!(result.text_content().contains("Verification workflow"));
    }
}
