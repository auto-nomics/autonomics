use std::sync::Arc;

use arc_swap::ArcSwap;

use super::{AgentRuntimeConfig, MEMORY_SCOPE_ID, MemoryConfig, MemoryStore, TursoKmsStorage};
use crate::pipeline::KnowledgeEvent;
use crate::store::KnowledgeQueue;

/// Process-wide KMS domain root.
///
/// The system owns durable storage and queueing. Agent-specific settings live
/// in [`KmsAgentHandle`], which shares this object instead of cloning storage
/// or pipeline state.
pub struct KmsSystem {
    default_config: ArcSwap<MemoryConfig>,
    store: Arc<dyn MemoryStore>,
    queue: Arc<dyn KnowledgeQueue>,
}

impl KmsSystem {
    #[must_use]
    pub fn new(config: MemoryConfig, storage: Arc<TursoKmsStorage>) -> Self {
        Self {
            default_config: ArcSwap::new(Arc::new(config)),
            store: Arc::clone(&storage) as Arc<_>,
            queue: Arc::clone(&storage) as Arc<_>,
        }
    }

    #[must_use]
    pub fn store(&self) -> Arc<dyn MemoryStore> {
        Arc::clone(&self.store)
    }

    #[must_use]
    pub fn queue(&self) -> Arc<dyn KnowledgeQueue> {
        Arc::clone(&self.queue)
    }

    #[must_use]
    pub fn effective_config(&self) -> MemoryConfig {
        MemoryConfig::clone(&self.default_config.load())
    }

    #[must_use]
    pub fn runtime_config(&self) -> AgentRuntimeConfig {
        let config = self.default_config.load();
        AgentRuntimeConfig {
            use_memory: config.use_memory,
            generate_memory: config.generate_memory,
            reasoning_effort: config.reasoning_effort,
        }
    }

    #[must_use]
    pub fn agent_handle(
        self: &Arc<Self>,
        runtime_config: AgentRuntimeConfig,
    ) -> Arc<KmsAgentHandle> {
        Arc::new(KmsAgentHandle::from_system(
            Arc::clone(self),
            runtime_config,
        ))
    }

    pub async fn submit_event(
        &self,
        event: KnowledgeEvent,
    ) -> Result<Vec<uuid::Uuid>, crate::Error> {
        let mut ids = Vec::new();
        for job in event.jobs() {
            ids.push(self.queue.enqueue(job).await?);
        }
        Ok(ids)
    }
}

/// Agent-scoped view over the process-wide [`KmsSystem`].
pub struct KmsAgentHandle {
    system: Arc<KmsSystem>,
    runtime_config: ArcSwap<AgentRuntimeConfig>,
}

impl KmsAgentHandle {
    #[must_use]
    pub fn new(config: MemoryConfig, storage: Arc<TursoKmsStorage>) -> Self {
        let system = Arc::new(KmsSystem::new(config, storage));
        let runtime_config = system.runtime_config();
        Self {
            system,
            runtime_config: ArcSwap::new(Arc::new(runtime_config)),
        }
    }

    #[must_use]
    pub fn from_system(system: Arc<KmsSystem>, runtime_config: AgentRuntimeConfig) -> Self {
        Self {
            system,
            runtime_config: ArcSwap::new(Arc::new(runtime_config)),
        }
    }

    #[must_use]
    pub fn system(&self) -> Arc<KmsSystem> {
        Arc::clone(&self.system)
    }

    #[must_use]
    pub fn store(&self) -> Arc<dyn MemoryStore> {
        self.system.store()
    }

    pub fn runtime_config(&self) -> AgentRuntimeConfig {
        **self.runtime_config.load()
    }

    pub fn set_runtime_config(&self, config: AgentRuntimeConfig) {
        self.runtime_config.store(Arc::new(config));
    }

    #[must_use]
    pub fn effective_memory_config(&self) -> MemoryConfig {
        let mut config = self.system.effective_config();
        let runtime = self.runtime_config();
        config.use_memory = runtime.use_memory;
        config.generate_memory = runtime.generate_memory;
        config.reasoning_effort = runtime.reasoning_effort;
        config
    }

    pub async fn submit_knowledge_event(
        &self,
        event: KnowledgeEvent,
    ) -> Result<Vec<uuid::Uuid>, crate::Error> {
        self.system.submit_event(event).await
    }

    pub async fn prompt_section(&self) -> Option<String> {
        let config = self.effective_memory_config();
        if !config.use_memory {
            return None;
        }
        let summary = self
            .system
            .store
            .get_summary(MEMORY_SCOPE_ID)
            .await
            .ok()
            .flatten()?;
        let mut summary = summary.summary_md.trim().to_string();
        if summary.is_empty() {
            return None;
        }
        if summary.len() > config.summary_max_bytes {
            let cut = summary
                .char_indices()
                .map(|(i, _)| i)
                .take_while(|i| i <= &config.summary_max_bytes)
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn agent_handles_isolate_runtime_config_over_one_system() {
        let storage = Arc::new(TursoKmsStorage::open_in_memory().await.unwrap());
        let system = Arc::new(KmsSystem::new(MemoryConfig::new(), storage));
        let enabled = system.agent_handle(AgentRuntimeConfig::new(true, true));
        let disabled = system.agent_handle(AgentRuntimeConfig::new(false, false));

        assert!(enabled.effective_memory_config().use_memory);
        assert!(!disabled.effective_memory_config().use_memory);
        assert!(enabled.effective_memory_config().generate_memory);
        assert!(!disabled.effective_memory_config().generate_memory);

        disabled.set_runtime_config(AgentRuntimeConfig::new(true, false));
        assert!(disabled.effective_memory_config().use_memory);
        assert!(!disabled.effective_memory_config().generate_memory);
        assert!(enabled.effective_memory_config().generate_memory);
    }
}
