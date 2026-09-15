//! Runtime read path and memory prompt injection.

use std::sync::Arc;

use arc_swap::ArcSwap;

use super::{AgentRuntimeConfig, MEMORY_SCOPE_ID, MemoryConfig, MemoryStore, SemanticGrounding};

pub struct MemoryBackend {
    config: ArcSwap<MemoryConfig>,
    pub store: Arc<dyn MemoryStore>,
    pub grounding: Option<Arc<dyn SemanticGrounding>>,
}

impl Clone for MemoryBackend {
    fn clone(&self) -> Self {
        let config = self.config.load();
        Self {
            config: ArcSwap::new(Arc::new(MemoryConfig::clone(&config))),
            store: Arc::clone(&self.store),
            grounding: self.grounding.clone(),
        }
    }
}

impl MemoryBackend {
    #[must_use]
    pub fn new(
        config: MemoryConfig,
        store: Arc<dyn MemoryStore>,
        grounding: Option<Arc<dyn SemanticGrounding>>,
    ) -> Self {
        Self {
            config: ArcSwap::new(std::sync::Arc::new(config)),
            store,
            grounding,
        }
    }

    #[must_use]
    pub fn runtime_config(&self) -> AgentRuntimeConfig {
        let config = self.config.load();
        AgentRuntimeConfig::new(config.use_memory, config.generate_memory)
    }

    pub fn set_runtime_config(&self, config: AgentRuntimeConfig) {
        let mut next = MemoryConfig::clone(&self.config.load());
        next.use_memory = config.use_memory;
        next.generate_memory = config.generate_memory;
        self.config.store(std::sync::Arc::new(next));
    }

    pub fn effective_memory_config(&self) -> MemoryConfig {
        MemoryConfig::clone(&self.config.load())
    }

    pub async fn prompt_section(&self) -> Option<String> {
        if !self.effective_memory_config().use_memory {
            return None;
        }
        let config = self.effective_memory_config();
        memory_prompt_section(
            self.store.as_ref(),
            MEMORY_SCOPE_ID,
            config.summary_max_bytes,
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
