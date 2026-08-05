//! Runtime host: process-level shared infrastructure + per-agent handles.
//!
//! [`RuntimeHost`] owns heavy shared resources (DataEngine, storage, file
//! system) that are created **once** per process. Individual agents are
//! spawned via [`RuntimeHost::spawn_agent`], which returns an [`AgentHandle`]
//! — a lightweight per-agent control struct (channels + task handle).
//!
//! This replaces the old [`AgentRuntime`](crate::AgentRuntime) pattern where
//! every agent duplicated the entire infrastructure.

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
use data_engine::runtime::{DataEngineClient, spawn_with_engine};
use datalake::Datalake;
use fs::OpendalFileStorage;
use thiserror::Error;
use tokio_util::sync::CancellationToken;

use crate::config::RuntimeConfig;
use crate::tools::DefaultToolSetError;

// ═══════════════════════════════════════════════════════════════════════
// Error
// ═══════════════════════════════════════════════════════════════════════

#[derive(Debug, Error)]
pub enum HostError {
    #[error("failed to build agent: {0}")]
    AgentBuild(#[from] AgentError),

    #[error("{0}")]
    Engine(#[from] data_engine::error::Error),

    #[error("OpenGWAS setup failed: {0}")]
    Opengwas(#[from] opengwas::OpengwasError),

    #[error("tool assembly failed: {0}")]
    ToolAssembly(#[from] DefaultToolSetError),

    #[error("agent storage error: {0}")]
    Storage(#[from] agentik_core::storage::StorageError),
}

pub type HostResult<T> = std::result::Result<T, HostError>;

// ═══════════════════════════════════════════════════════════════════════
// SharedInfra — process-level shared resources
// ═══════════════════════════════════════════════════════════════════════

/// Process-level shared infrastructure created once and reused by all agents.
///
/// Cloning is cheap — every field is `Arc`-backed.
#[derive(Clone)]
pub struct SharedInfra {
    pub engine_client: DataEngineClient,
    pub file_storage: Arc<OpendalFileStorage>,
    pub datalake: Arc<Datalake>,
    pub storage: Arc<dyn AgentStorage>,
    /// The tokio runtime handle (for spawning agent tasks).
    pub runtime_handle: tokio::runtime::Handle,
    /// Keep the engine task alive as long as infra is alive.
    _engine_handle: Arc<tokio::task::JoinHandle<()>>,
}

impl SharedInfra {
    /// Open all shared resources from a global [`RuntimeConfig`].
    ///
    /// The config's `data_dir`, `state_dir`, `agent_db`, and feature flags
    /// for Iceberg / DAG history are consumed here. Per-agent settings
    /// (identity, prompts, tool flags) are **not** read — those are passed
    /// to [`RuntimeHost::spawn_agent`] instead.
    pub async fn open(config: &RuntimeConfig) -> HostResult<Self> {
        let file_storage = Arc::new(OpendalFileStorage::new(&config.data_dir));

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
                    tracing::info!(
                        path = %history_db.display(),
                        "DAG history store opened"
                    );
                    engine.with_history(history)
                }
                Err(e) => {
                    tracing::warn!(
                        path = %history_db.display(),
                        error = %e,
                        "failed to open DAG history store; history/ref tools disabled"
                    );
                    engine
                }
            };
        }

        let (engine_client, engine_handle) = spawn_with_engine(engine);

        // ── Datalake ─────────────────────────────────────────────────
        let datalake = Arc::new(Datalake::new());

        // ── Agent storage ────────────────────────────────────────────
        let storage: Arc<dyn AgentStorage> = match TursoAgentStorage::open(&config.agent_db).await {
            Ok(s) => {
                tracing::info!(path = %config.agent_db.display(), "agent storage opened");
                Arc::new(s)
            }
            Err(e) => {
                return Err(HostError::Storage(e));
            }
        };

        Ok(Self {
            engine_client,
            file_storage,
            datalake,
            storage,
            runtime_handle: tokio::runtime::Handle::current(),
            _engine_handle: Arc::new(engine_handle),
        })
    }
}

// ═══════════════════════════════════════════════════════════════════════
// AgentHandle — per-agent control struct
// ═══════════════════════════════════════════════════════════════════════

/// Control handle for one running agent.
///
/// Cheap to move; owns the event receiver and cancellation token. When
/// dropped, the agent's tokio task is **not** automatically killed — call
/// [`shutdown`](Self::shutdown) explicitly.
pub struct AgentHandle {
    pub agent_id: uuid::Uuid,
    pub name: String,
    internal_tx: tokio::sync::mpsc::UnboundedSender<InternalEvent>,
    event_rx: tokio::sync::mpsc::UnboundedReceiver<AgentEvent>,
    agent_task: tokio::task::JoinHandle<()>,
    cancel_token: CancellationToken,
    model: Arc<ArcSwapOption<Model>>,
}

impl AgentHandle {
    pub fn send_message(&self, text: String) {
        let _ = self
            .internal_tx
            .send(InternalEvent::MessageInject(vec![ContentBlock::Text { text }]));
    }

    pub fn cancel(&mut self) {
        self.cancel_token.cancel();
        let new_token = CancellationToken::new();
        let _ = self
            .internal_tx
            .send(InternalEvent::ResetCancelToken(new_token.clone()));
        self.cancel_token = new_token;
    }

    /// Force-stop the agent: abort the background task.
    pub fn shutdown(&mut self) {
        let _ = self.internal_tx.send(InternalEvent::Shutdown);
        self.agent_task.abort();
    }

    pub fn poll_event(&mut self) -> Option<AgentEvent> {
        self.event_rx.try_recv().ok()
    }

    pub async fn recv_event(&mut self) -> Option<AgentEvent> {
        self.event_rx.recv().await
    }

    pub fn model_handle(&self) -> &Arc<ArcSwapOption<Model>> {
        &self.model
    }

    /// Hot-swap the model for this specific agent. The agent's run loop
    /// picks up the new model on its next LLM request.
    pub fn set_model(&self, model: Model) {
        self.model.store(Some(Arc::new(model)));
    }
}

// ═══════════════════════════════════════════════════════════════════════
// RuntimeHost — top-level multi-agent manager
// ═══════════════════════════════════════════════════════════════════════

/// Top-level runtime that owns shared infrastructure and manages multiple
/// agents.
///
/// Created once per process via [`RuntimeHost::open`]. Individual agents
/// are spawned via [`RuntimeHost::spawn_agent`]. The TUI holds one
/// `RuntimeHost` and switches between agents using the returned
/// [`AgentHandle`]s.
///
/// # Example
///
/// ```ignore
/// let host = RuntimeHost::open(&config).await?;
/// let default_model = Arc::new(ArcSwapOption::from_pointee(Some(model)));
///
/// // Spawn agent from a profile
/// let mut handle = host.spawn_agent(
///     &profile,
///     default_model.clone(),
///     None,  // no per-agent model override
/// ).await?;
///
/// handle.send_message("hello".into());
/// while let Some(ev) = handle.recv_event().await { /* ... */ }
/// ```
pub struct RuntimeHost {
    infra: SharedInfra,
    /// Bib DB path (shared across all agents).
    bib_db_path: std::path::PathBuf,
}

impl Clone for RuntimeHost {
    fn clone(&self) -> Self {
        Self {
            infra: self.infra.clone(),
            bib_db_path: self.bib_db_path.clone(),
        }
    }
}

impl RuntimeHost {
    /// Open shared infrastructure from a global config.
    pub async fn open(config: &RuntimeConfig) -> HostResult<Self> {
        let infra = SharedInfra::open(config).await?;
        Ok(Self {
            infra,
            bib_db_path: config.bib_db_path.clone(),
        })
    }

    /// Returns a reference to the shared storage.
    pub fn storage(&self) -> &Arc<dyn AgentStorage> {
        &self.infra.storage
    }

    /// Returns a clone of the shared infra (for advanced use).
    pub fn infra(&self) -> SharedInfra {
        self.infra.clone()
    }

    /// Spawn a new agent from an [`AgentProfile`](agentik_core::AgentProfile).
    ///
    /// - `global_model`: the process-wide default model. Used when
    ///   `model_override` is `None`.
    /// - `model_override`: an optional per-agent model (e.g. resolved from
    ///   the profile's `preferred_model`). When `Some`, the agent gets its
    ///   own `ArcSwapOption` slot instead of sharing the global one.
    /// - `agent_name`: the **unique** name for this agent instance. The
    ///   profile name is used for configuration (identity, prompts, tools)
    ///   but the agent's identity is independent — multiple agents can
    ///   share the same profile with different names.
    pub async fn spawn_agent(
        &self,
        agent_name: &str,
        profile: &agentik_core::AgentProfile,
        global_model: Arc<ArcSwapOption<Model>>,
        model_override: Option<Model>,
    ) -> HostResult<AgentHandle> {
        let (event_tx, event_rx) = tokio::sync::mpsc::unbounded_channel();
        let cancel_token = CancellationToken::new();

        // Resolve model: per-agent override, or a fresh independent slot
        // cloned from the current global model. Using a dedicated ArcSwapOption
        // for each agent is critical — sharing the global Arc would cause
        // `set_model` on one agent to mutate the model for all agents.
        let model = match model_override {
            Some(m) => Arc::new(ArcSwapOption::from_pointee(Some(m))),
            None => Arc::new(ArcSwapOption::from_pointee(
                global_model.load_full().as_deref().cloned(),
            )),
        };

        // ── Assemble tools from profile flags ──────────────────────
        let tool_list = self.tools_from_profile(profile).await?;

        // ── Build agent ─────────────────────────────────────────────
        let config_json = serde_json::to_value(profile).unwrap_or_default();
        let storage = self.infra.storage.clone();

        let mut builder = Agent::builder()
            .with_model(model.clone())
            .with_agent_event_tx(event_tx)
            .with_name(agent_name)
            .with_config_json(config_json)
            .with_system_prompt_identity(&profile.agent_identity)
            .with_storage(storage.clone());

        // System prompt: profile override or built-in default.
        if let Some(ref prompt) = profile.system_prompt {
            builder = builder.with_system_prompt_section(prompt);
        } else {
            builder = builder.with_system_prompt_section(
                crate::config::default_system_prompt(),
            );
        }

        builder = builder
            .with_tools(tool_list)
            .with_cancel_token(cancel_token.clone());

        // ── Restore from storage if this agent name already exists ──
        if let Ok(Some(record)) = storage.get_agent_by_name(agent_name).await {
            tracing::info!(
                agent = %agent_name,
                agent_id = %record.id,
                "restoring agent from storage"
            );
            builder = builder.with_id(record.id);

            if let Ok(memory) = restore_memory(storage.as_ref(), record.id).await {
                builder = builder.with_memory(memory);
                tracing::info!("memory restored from snapshot + WAL");
            }
        }

        let mut agent = builder.build().await?;
        let agent_id = agent.id();
        let agent_name = agent.name().to_string();
        let internal_tx = agent.internal_event_tx();
        let model_handle = agent.model_handle().clone();

        let agent_task = self.infra.runtime_handle.spawn(async move {
            agent.run().await;
        });

        Ok(AgentHandle {
            agent_id,
            name: agent_name,
            internal_tx,
            event_rx,
            agent_task,
            cancel_token,
            model: model_handle,
        })
    }

    /// Assemble the tool set for a profile, respecting its feature flags.
    ///
    /// Each agent gets a **per-session** `DataEngineClient` via
    /// [`DataEngineClient::with_session`], so its DAG graph is isolated
    /// from other agents' DAGs while sharing the same engine infrastructure
    /// (NodeRegistry, RuntimeEnv, DagHistory).
    async fn tools_from_profile(
        &self,
        profile: &agentik_core::AgentProfile,
    ) -> HostResult<Vec<agentik_core::tools::ToolRegistration>> {
        use agentik_core::tools::ToolRegistration;
        use crate::tools::*;

        let file_storage = self.infra.file_storage.clone();
        let datalake = self.infra.datalake.clone();
        // Create a per-agent engine session — independent DAG, shared infra.
        let engine_client = self.infra.engine_client.with_session(&profile.name);

        // Filesystem / shell tools — always enabled.
        let mut tools: Vec<ToolRegistration> = fs::vbash_registrations(file_storage.clone());

        if profile.enable_opengwas {
            match opengwas_tools_with_token(file_storage.clone(), None) {
                Ok(t) => tools.extend(t),
                Err(e) => tracing::warn!(error = %e, "OpenGWAS tools disabled"),
            }
        }

        if profile.enable_opentargets {
            tools.extend(opentargets_tools());
        }

        if profile.enable_gwascatalog {
            tools.extend(gwascatalog_tools(file_storage));
        }

        tools.extend(datalake_tools(datalake));
        tools.extend(data_engine_tools::registrations(Arc::new(engine_client)));

        if profile.enable_bibliography {
            match bib_tools(&self.bib_db_path.to_string_lossy()).await {
                Ok(t) => tools.extend(t),
                Err(e) => tracing::warn!(error = %e, "bibliography tools disabled"),
            }
        }

        Ok(tools)
    }
}
