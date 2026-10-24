use std::sync::Arc;

use agentik_proc::tool;
use agentik_sdk::types::tools::ToolResult;
use async_trait::async_trait;
use uuid::Uuid;

use crate::memory::{KmsAgentHandle, MEMORY_SCOPE_ID};
use crate::tools::{ToolError, ToolFunction};

use super::util::store_error;

const DEFAULT_READ_LINES: usize = 200;
const MAX_READ_LINES: usize = 1_000;

#[tool(
    name = "memory_read",
    description = "Read the current memory summary or one active knowledge node by UUID."
)]
pub struct MemoryReadInput {
    #[desc = "'summary' for the compact summary, or a knowledge node UUID returned by memory_search."]
    pub target: String,
    #[desc = "Starting 1-indexed line. Default 1."]
    pub offset: Option<usize>,
    #[desc = "Maximum lines returned. Default 200; max 1000."]
    pub limit: Option<usize>,
}

pub struct MemoryReadTool {
    backend: Arc<KmsAgentHandle>,
}

impl MemoryReadTool {
    pub fn new(backend: Arc<KmsAgentHandle>) -> Self {
        Self { backend }
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
                .store()
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
        let node = self
            .backend
            .store()
            .get_knowledge(entry_id)
            .await
            .map_err(store_error)?
            .ok_or_else(|| ToolError::ValidationFailed {
                message: format!("knowledge node {entry_id} not found"),
            })?;
        let lines: Vec<&str> = node.body_md.lines().skip(offset - 1).take(limit).collect();
        Ok(ToolResult::success_json(serde_json::json!({
            "kind": "knowledge_node",
            "id": node.id,
            "knowledge_path": node.knowledge_path.as_str(),
            "title": node.title,
            "status": node.status,
            "confidence": node.confidence,
            "structural": node.structural,
            "offset": offset,
            "lines": lines,
            "returned": lines.len(),
        })))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use uuid::Uuid;

    use crate::memory::{
        KmsAgentHandle, Knowledge, KnowledgePath, KnowledgeStore, MEMORY_SCOPE_ID, MemoryConfig,
        MemoryStore, TursoKmsStorage,
    };

    use super::*;

    #[tokio::test]
    async fn reads_summary_and_entry() {
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

        let result = MemoryReadTool::new(Arc::clone(&backend))
            .run(MemoryReadInput {
                target: "summary".into(),
                offset: None,
                limit: None,
            })
            .await
            .unwrap();
        assert!(result.text_content().contains("Run tests."));

        let node = store
            .list_knowledge(10)
            .await
            .unwrap()
            .into_iter()
            .find(|node| !node.structural)
            .unwrap();
        let result = MemoryReadTool::new(backend)
            .run(MemoryReadInput {
                target: node.id.to_string(),
                offset: None,
                limit: None,
            })
            .await
            .unwrap();
        assert!(result.text_content().contains("targeted tests"));
    }
}
