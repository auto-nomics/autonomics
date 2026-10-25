use std::sync::{Arc, OnceLock, Weak};

use agentik_sdk::model::Model;
use arc_swap::ArcSwap;
use arc_swap::ArcSwapOption;

use super::{AgentRuntimeConfig, MemoryConfig, MemoryStore, TursoKmsStorage};
use crate::pipeline::{
    ConsolidationWorkerConfig, ConsolidationWorkerHandle, KnowledgeEvent, KnowledgeJob,
    KnowledgeJobKind, KnowledgePipeline, SessionExtractWorkerConfig, SessionExtractWorkerHandle,
    start_consolidation_worker, start_session_extract_worker,
};
use crate::source::MemorySource;
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
    pipeline: Arc<KnowledgePipeline>,
    model: Arc<ArcSwapOption<Model>>,
    session_extract_worker: OnceLock<SessionExtractWorkerHandle>,
    consolidation_worker: OnceLock<ConsolidationWorkerHandle>,
}

impl KmsSystem {
    /// Build the process-wide KMS root and start its resident consumers.
    ///
    /// The returned `Arc` is deliberate: the session-extract pipeline needs a
    /// shared view of config and storage. The model slot starts empty so the
    /// host can start before its default model is resolved; extraction jobs
    /// retry while the slot is empty.
    #[must_use]
    pub async fn new(
        config: MemoryConfig,
        storage: Arc<TursoKmsStorage>,
        source: Arc<dyn MemorySource>,
    ) -> Arc<Self> {
        let model = Arc::new(ArcSwapOption::empty());
        // The pipeline owns KMS weakly: KmsSystem is the process root and owns
        // the pipeline strongly, so a worker can outlive a shutdown without
        // leaking the entire system through a reference cycle.
        let system = Arc::new_cyclic(|weak: &Weak<Self>| {
            let pipeline = Arc::new(KnowledgePipeline::new(
                Arc::clone(&storage) as Arc<_>,
                Arc::clone(&source),
                Weak::clone(weak),
                Arc::clone(&model),
            ));
            Self {
                default_config: ArcSwap::new(Arc::new(config)),
                store: Arc::clone(&storage) as Arc<_>,
                queue: Arc::clone(&storage) as Arc<_>,
                pipeline,
                model: Arc::clone(&model),
                session_extract_worker: OnceLock::new(),
                consolidation_worker: OnceLock::new(),
            }
        });
        let worker = start_session_extract_worker(
            Arc::clone(&system.pipeline),
            SessionExtractWorkerConfig::default(),
        );
        let _ = system.session_extract_worker.set(worker);
        let consolidation_worker = start_consolidation_worker(
            Arc::clone(&system.pipeline),
            ConsolidationWorkerConfig::default(),
        );
        let _ = system.consolidation_worker.set(consolidation_worker);
        system
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
    pub fn pipeline(&self) -> Arc<KnowledgePipeline> {
        Arc::clone(&self.pipeline)
    }

    #[must_use]
    pub fn model_slot(&self) -> Arc<ArcSwapOption<Model>> {
        Arc::clone(&self.model)
    }

    #[must_use]
    pub fn session_extract_worker(&self) -> Option<SessionExtractWorkerHandle> {
        self.session_extract_worker.get().cloned()
    }

    #[must_use]
    pub fn consolidation_worker(&self) -> Option<ConsolidationWorkerHandle> {
        self.consolidation_worker.get().cloned()
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
        self.submit_jobs(event.jobs()).await
    }

    /// Enqueue durable jobs and wake only the consumers that accept them.
    ///
    /// External events and stage continuations both funnel through this
    /// method so queue insertion and worker notification cannot diverge.
    pub async fn submit_jobs(
        &self,
        jobs: Vec<KnowledgeJob>,
    ) -> Result<Vec<uuid::Uuid>, crate::Error> {
        let mut ids = Vec::new();
        for job in jobs {
            ids.push(self.queue.enqueue(job.clone()).await?);
            self.wake_for_job(job);
        }
        Ok(ids)
    }

    fn wake_for_job(&self, job: KnowledgeJob) {
        match job.kind() {
            KnowledgeJobKind::SessionExtract => {
                if let Some(worker) = self.session_extract_worker() {
                    worker.wake();
                }
            }
            KnowledgeJobKind::ConsolidateKnowledge => {
                if let Some(worker) = self.consolidation_worker() {
                    worker.wake();
                }
            }
        }
    }

    pub fn shutdown(&self) {
        if let Some(worker) = self.session_extract_worker() {
            worker.request_shutdown();
        }
        if let Some(worker) = self.consolidation_worker() {
            worker.request_shutdown();
        }
    }
}

/// Agent-scoped view over the process-wide [`KmsSystem`].
pub struct KmsAgentHandle {
    system: Arc<KmsSystem>,
    runtime_config: ArcSwap<AgentRuntimeConfig>,
}

impl KmsAgentHandle {
    #[must_use]
    pub async fn new(
        config: MemoryConfig,
        storage: Arc<TursoKmsStorage>,
        source: Arc<dyn MemorySource>,
    ) -> Self {
        let system = KmsSystem::new(config, storage, source).await;
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
        let summary = self.system.store.get_summary().await.ok().flatten()?;
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
        let system = KmsSystem::new(
            MemoryConfig::new(),
            storage,
            Arc::new(crate::EmptyMemorySource),
        )
        .await;
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
        system.shutdown();
    }

    #[tokio::test]
    async fn system_starts_and_shuts_down_resident_workers() {
        let storage = Arc::new(TursoKmsStorage::open_in_memory().await.unwrap());
        let system = KmsSystem::new(
            MemoryConfig::new(),
            storage,
            Arc::new(crate::EmptyMemorySource),
        )
        .await;
        let worker = system.session_extract_worker().expect("worker mounted");
        let consolidation_worker = system.consolidation_worker().expect("worker mounted");
        system.shutdown();

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while worker.status().running || consolidation_worker.status().running {
            assert!(
                std::time::Instant::now() < deadline,
                "resident knowledge workers did not stop"
            );
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }
}
