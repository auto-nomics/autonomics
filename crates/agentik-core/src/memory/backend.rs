//! Runtime read path and memory prompt injection.

use std::sync::Arc;

use super::{MEMORY_SCOPE_ID, MemoryConfig, MemoryStore, SemanticGrounding};

#[derive(Clone)]
pub struct MemoryBackend {
    pub config: MemoryConfig,
    pub store: Arc<dyn MemoryStore>,
    pub grounding: Option<Arc<dyn SemanticGrounding>>,
}

impl MemoryBackend {
    #[must_use]
    pub fn new(
        config: MemoryConfig,
        store: Arc<dyn MemoryStore>,
        grounding: Option<Arc<dyn SemanticGrounding>>,
    ) -> Self {
        Self {
            config,
            store,
            grounding,
        }
    }

    pub async fn prompt_section(&self) -> Option<String> {
        memory_prompt_section(
            self.store.as_ref(),
            MEMORY_SCOPE_ID,
            self.config.summary_max_bytes,
        )
        .await
    }
}

pub async fn memory_prompt_section(
    store: &dyn MemoryStore,
    scope_id: uuid::Uuid,
    max_bytes: usize,
) -> Option<String> {
    let summary = store.get_summary(scope_id).await.ok()?;
    let summary = summary?.summary_md.trim().to_string();
    if summary.is_empty() {
        return None;
    }
    let mut summary = summary;
    if summary.len() > max_bytes {
        let cut = summary
            .char_indices()
            .map(|(i, _)| i)
            .take_while(|i| i <= &max_bytes)
            .last()
            .unwrap_or(0);
        summary = format!("{}\n[MEMORY_SUMMARY_TRUNCATED]", &summary[..cut]);
    }
    Some(format!(
        "## Persistent memory\n\n\
         The compact summary below is loaded from prior sessions. Use it when \
         relevant, and use `memory_search`, `memory_read`, or `memory_list` for \
         progressive disclosure. Explicit current instructions and verified \
         current-state evidence override memory. Memory-derived facts must not \
         be presented as current facts without verification.\n\n\
         ### MEMORY_SUMMARY\n\n{summary}\n"
    ))
}
