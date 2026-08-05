use std::sync::Arc;

use agentik_core::Agent;
use agentik_core::agent::InternalEvent;
use agentik_core::error::AgentError;
use agentik_core::storage::{AgentStorage, restore_memory};
use agentik_core::TursoAgentStorage;
use agentik_sdk::model::Model;
use agentik_sdk::types::{AgentEvent, ContentBlock};
use arc_swap::ArcSwapOption;
use data_engine::dag::DagHistory;
use data_engine::data_engine::DataEngine;
use data_engine::runtime::spawn_with_engine;
use datalake::Datalake;
use fs::OpendalFileStorage;
use thiserror::Error;
use tokio_util::sync::CancellationToken;

use crate::config::RuntimeConfig;

/// Errors that can occur while building or driving an [`AgentRuntime`].
#[derive(Debug, Error)]
pub enum RuntimeError {
    /// The agent could not be assembled (e.g. model pool misconfiguration,
    /// missing required tools, internal initialization failure).
    #[error("failed to build agent: {0}")]
    AgentBuild(#[from] AgentError),

    #[error("{0}")]
    Engine(#[from] data_engine::error::Error),

    #[error("OpenGWAS setup failed: {0}")]
    Opengwas(#[from] opengwas::OpengwasError),

    #[error("tool assembly failed: {0}")]
    ToolAssembly(#[from] crate::tools::DefaultToolSetError),

    #[error("agent storage error: {0}")]
    Storage(#[from] agentik_core::storage::StorageError),
}

pub type Result<T> = std::result::Result<T, RuntimeError>;

pub struct AgentRuntime {
    internal_tx: tokio::sync::mpsc::UnboundedSender<InternalEvent>,
    event_rx: tokio::sync::mpsc::UnboundedReceiver<AgentEvent>,
    _engine_handle: tokio::task::JoinHandle<()>,
    /// Handle for the spawned agent task, so we can abort it on forced shutdown.
    agent_handle: tokio::task::JoinHandle<()>,
    cancel_token: CancellationToken,
    /// Shared storage handle (when persistence is enabled).
    storage: Option<Arc<dyn AgentStorage>>,
    /// The model slot used by this agent's run loop. When the agent uses the
    /// global default model, this Arc is shared with the caller's handle.
    /// When the agent has a per-agent model (from profile.preferred_model),
    /// this is a dedicated `ArcSwapOption`.
    model: Arc<ArcSwapOption<Model>>,
}

impl AgentRuntime {
    /// Create a runtime with the default [`RuntimeConfig`] (env vars +
    /// hard-coded defaults, same behaviour as before).
    pub fn new(
        runtime: &tokio::runtime::Runtime,
        model: Arc<ArcSwapOption<Model>>,
    ) -> Result<Self> {
        Self::with_config(runtime, model, RuntimeConfig::default())
    }

    /// Create a runtime with an explicit [`RuntimeConfig`].
    ///
    /// This is the primary entry point for multi-agent setups — each agent
    /// gets its own `RuntimeConfig` with independent storage, DBs, prompts,
    /// and feature flags.
    ///
    /// When `config.agent_db` points to an existing database with a
    /// registered agent, the agent's memory is automatically restored from
    /// the latest snapshot + WAL replay. Otherwise a fresh agent is created
    /// and registered.
    pub fn with_config(
        runtime: &tokio::runtime::Runtime,
        model: Arc<ArcSwapOption<Model>>,
        config: RuntimeConfig,
    ) -> Result<Self> {
        tracing::info!(name = ?config.name, "{}", config.summary());

        let (event_tx, event_rx) = tokio::sync::mpsc::unbounded_channel();
        let cancel_token = CancellationToken::new();

        let file_storage = Arc::new(OpendalFileStorage::new(&config.data_dir));

        let (internal_tx, engine_handle, agent_handle, storage, model_handle) = runtime.block_on(async {
            // ── DataEngine ───────────────────────────────────────────────
            let mut engine_builder =
                DataEngine::builder().register_opendal_fs(file_storage.clone())?;

            if config.enable_iceberg {
                engine_builder = engine_builder.register_iceberg().await?;
            }

            let mut engine = engine_builder.build();

            // ── DAG history ──────────────────────────────────────────────
            if config.enable_dag_history {
                let history_db = &config.dag_history_db;
                if let Some(parent) = history_db.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                engine = match DagHistory::open(history_db).await {
                    Ok(history) => {
                        tracing::info!(path = %history_db.display(), "DAG history store opened");
                        engine.with_history(history)
                    }
                    Err(e) => {
                        tracing::warn!(path = %history_db.display(), error = %e, "failed to open DAG history store; history/ref tools disabled");
                        engine
                    }
                };
            }

            let (data_engine_client, engine_handle) = spawn_with_engine(engine);

            // ── Tools ────────────────────────────────────────────────────
            let datalake = Arc::new(Datalake::new());

            let tool_list = crate::tools::tool_set_from_config(
                file_storage,
                datalake,
                Arc::new(data_engine_client),
                &config,
            )
            .await?;

            // ── Agent storage (persistence) ──────────────────────────────
            let storage: Option<Arc<dyn AgentStorage>> =
                match TursoAgentStorage::open(&config.agent_db).await {
                    Ok(s) => {
                        tracing::info!(path = %config.agent_db.display(), "agent storage opened");
                        Some(Arc::new(s))
                    }
                    Err(e) => {
                        tracing::warn!(path = %config.agent_db.display(), error = %e, "agent storage failed; persistence disabled");
                        None
                    }
                };

            // ── Restore or create agent ──────────────────────────────────
            let config_json = serde_json::to_value(&config).unwrap_or_default();

            let mut builder = Agent::builder()
                .with_model(model)
                .with_agent_event_tx(event_tx)
                .with_name(&config.name)
                .with_config_json(config_json)
                .with_system_prompt_identity(&config.agent_identity)
                .with_system_prompt_section(config.system_prompt_or_default())
                .with_tools(tool_list)
                .with_cancel_token(cancel_token.clone());

            if let Some(ref storage) = storage {
                builder = builder.with_storage(storage.clone());

                // Try to restore from existing registry.
                if let Ok(Some(record)) = storage.get_agent_by_name(&config.name).await {
                    tracing::info!(agent = %config.name, agent_id = %record.id, "restoring agent from storage");
                    builder = builder.with_id(record.id);

                    if let Ok(memory) = restore_memory(storage.as_ref(), record.id).await {
                        builder = builder.with_memory(memory);
                        tracing::info!("memory restored from snapshot + WAL");
                    }
                }
            }

            let mut agent = builder.build().await?;

            let tx = agent.internal_event_tx();
            let model_handle = agent.model_handle().clone();

            let agent_handle = tokio::spawn(async move {
                agent.run().await;
            });

            Ok::<_, RuntimeError>((tx, engine_handle, agent_handle, storage, model_handle))
        })?;

        Ok(Self {
            internal_tx,
            event_rx,
            _engine_handle: engine_handle,
            agent_handle,
            cancel_token,
            storage,
            model: model_handle,
        })
    }

    pub fn send_message(&self, text: String) {
        let _ = self
            .internal_tx
            .send(InternalEvent::MessageInject(vec![ContentBlock::Text {
                text,
            }]));
    }

    pub fn cancel(&mut self) {
        self.cancel_token.cancel();
        let new_token = CancellationToken::new();
        let _ = self
            .internal_tx
            .send(InternalEvent::ResetCancelToken(new_token.clone()));
        self.cancel_token = new_token;
    }

    /// Force-stop the agent: abort the background task and drop the event
    /// channel so the TUI can exit immediately.
    pub fn shutdown(&mut self) {
        let _ = self.internal_tx.send(InternalEvent::Shutdown);
        self.agent_handle.abort();
    }

    pub fn poll_event(&mut self) -> Option<AgentEvent> {
        self.event_rx.try_recv().ok()
    }

    /// Async receive: suspends until an agent event arrives (or the channel closes).
    pub async fn recv_event(&mut self) -> Option<AgentEvent> {
        self.event_rx.recv().await
    }

    /// Returns the shared storage handle, if persistence is enabled.
    pub fn storage(&self) -> Option<&Arc<dyn AgentStorage>> {
        self.storage.as_ref()
    }

    /// Returns a handle to this agent's model slot.
    ///
    /// Callers can `.store(Some(Arc::new(model)))` to hot-swap the model.
    /// When the agent uses the global default, this Arc is the same object
    /// the caller passed in. When the agent has a per-agent model, this is
    /// a dedicated slot.
    pub fn model_handle(&self) -> &Arc<ArcSwapOption<Model>> {
        &self.model
    }
}
