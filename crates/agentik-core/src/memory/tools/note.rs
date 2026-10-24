use std::sync::Arc;

use agentik_proc::tool;
use agentik_sdk::types::tools::ToolResult;
use async_trait::async_trait;

use crate::memory::{KmsAgentHandle, KnowledgeEvent, redact_secrets};
use crate::tools::{ToolError, ToolFunction};

use super::util::store_error;

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

pub struct MemoryNoteTool {
    backend: Arc<KmsAgentHandle>,
}

impl MemoryNoteTool {
    pub fn new(backend: Arc<KmsAgentHandle>) -> Self {
        Self { backend }
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
            .store()
            .insert_note(&note_slug(input.slug.as_deref()), &content)
            .await
            .map_err(store_error)?;
        self.backend
            .submit_knowledge_event(KnowledgeEvent::MemoryNoteCreated { note_id: note.id })
            .await
            .map_err(|error| ToolError::ExecutionFailed {
                source: Box::new(error),
            })?;
        Ok(ToolResult::success_json(serde_json::json!({
            "id": note.id,
            "slug": note.slug,
            "status": "pending",
        })))
    }
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

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use crate::memory::{KmsAgentHandle, MemoryConfig, MemoryStore, TursoKmsStorage};

    use super::*;

    #[tokio::test]
    async fn queues_redacted_note() {
        let store = Arc::new(TursoKmsStorage::open_in_memory().await.unwrap());
        let backend = Arc::new(KmsAgentHandle::new(MemoryConfig::new(), Arc::clone(&store)));

        let result = MemoryNoteTool::new(backend)
            .run(MemoryNoteInput {
                content: "token=abc\nPrefer concise summaries.".into(),
                slug: Some("User Preference!".into()),
            })
            .await
            .unwrap();
        assert!(result.text_content().contains("user-preference"));

        let notes = store.list_pending_notes().await.unwrap();
        assert_eq!(notes.len(), 1);
        assert!(notes[0].content.contains("REDACTED"));
    }
}
