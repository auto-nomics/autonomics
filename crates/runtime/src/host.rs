//! Runtime host: process-level shared infrastructure + per-agent handles.
//!
//! [`RuntimeHost`] owns heavy shared resources (DataEngine, storage, file
//! system) that are created **once** per process. It also owns a persistent
//! [`AgentNetwork`] for multi-agent topology routing.
//!
//! Individual agents are spawned via [`RuntimeHost::spawn_agent`], which
//! returns an [`AgentHandle`] — a lightweight per-agent control struct
//! (channels + task handle). Agents can be registered with the host's
//! internal registry for multiplexed event access via
//! [`recv_any`](Self::recv_any) and topology-aware message routing via
//! [`AgentNetwork`].

use std::collections::HashMap;
use std::sync::Arc;

use agentik_core::Agent;
use agentik_core::TursoAgentStorage;
use agentik_core::agent::InternalEvent;
use agentik_core::error::AgentError;
use agentik_core::storage::{AgentStorage, restore_memory};
use agentik_network::{AgentNetwork, EdgeTrigger, NodeSpec, RoutingAction, TerminationSpec};
use agentik_sdk::model::Model;
use agentik_sdk::types::{AgentEvent, ContentBlock};
use arc_swap::ArcSwapOption;
use data_engine::dag::DagHistory;
use data_engine::data_engine::DataEngine;
use data_engine::runtime::{DataEngineClient, DataEngineManager};
use datalake::Datalake;
use fs::OpendalFileStorage;
use thiserror::Error;
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use tokio::task::JoinHandle;
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

    #[error("bibliography shared init failed: {0}")]
    BibShared(#[from] bib_base::Error),

    #[error("writing system init failed: {0}")]
    WritingShared(#[from] writing_base::Error),

    #[error("{0}")]
    Other(String),
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
    /// Per-agent session manager — each agent gets its own DAG actor task,
    /// fully isolated from other agents. Heavy infrastructure (NodeRegistry,
    /// DagHistory, RuntimeEnv) is shared via `Arc`.
    pub engine_manager: Arc<DataEngineManager>,
    pub file_storage: Arc<OpendalFileStorage>,
    pub datalake: Arc<Datalake>,
    pub storage: Arc<dyn AgentStorage>,
    /// Bibliography storage + literature gateway, opened **once** per
    /// process and shared by every spawned agent.
    pub bib: Arc<bib_base::BibShared>,
    /// LaTeX writing system (store + optional engine), opened **once** per
    /// process. Reuses `bib` for citation resolution when available.
    pub writing: Arc<writing_base::WritingShared>,
    /// The tokio runtime handle (for spawning agent tasks).
    pub runtime_handle: tokio::runtime::Handle,
    /// Optional host control for agent tools. Set by RuntimeHost when
    /// available. When `Some`, spawned agents receive host management tools
    /// (spawn_agent, send_to_agent, connect_agents, etc.).
    pub host_control: Option<crate::control::HostControl>,
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
        let mut engine_builder = DataEngine::builder().register_opendal_fs(file_storage.clone())?;

        if config.enable_iceberg {
            match engine_builder.register_iceberg().await {
                Ok(()) => {
                    tracing::info!("iceberg datalake registered");
                }
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        "failed to connect to iceberg datalake; \
                         iceberg/datalake tools disabled — \
                         agent spawn/resume will still work"
                    );
                }
            }
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

        let engine_manager = Arc::new(DataEngineManager::new(engine));

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

        let bib = Arc::new(
            bib_base::BibShared::open_with(&config.bib_db_path, config.bib_http.clone())
                .await?,
        );

        let writing = Arc::new(
            writing_base::WritingShared::open_with(
                &config.writing_db_path.to_string_lossy(),
                Some(bib.clone()),
            )
            .await?,
        );

        Ok(Self {
            engine_manager,
            file_storage,
            datalake,
            storage,
            runtime_handle: tokio::runtime::Handle::current(),
            bib,
            writing,
            host_control: None,
        })
    }

    /// Spawn a new agent from an [`AgentProfile`](agentik_core::AgentProfile).
    ///
    /// This method only needs [`SharedInfra`] — it does not touch the
    /// topology network or agent registry. Callers can clone `SharedInfra`
    /// (cheap, all `Arc`) into async tasks.
    pub async fn spawn_agent(
        &self,
        agent_path: &agentik_types::AgentPath,
        profile: &agentik_core::AgentProfile,
        global_model: Arc<ArcSwapOption<Model>>,
        model_override: Option<Model>,
    ) -> HostResult<AgentHandle> {
        let (event_tx, event_rx) = tokio::sync::mpsc::unbounded_channel();
        let cancel_token = CancellationToken::new();

        let model = match model_override {
            Some(m) => Arc::new(ArcSwapOption::from_pointee(Some(m))),
            None => Arc::new(ArcSwapOption::from_pointee(
                global_model.load_full().as_deref().cloned(),
            )),
        };

        let tool_list = self.tools_from_profile(agent_path, profile).await?;

        let config_json = serde_json::to_value(profile).unwrap_or_default();
        let storage = self.storage.clone();

        let mut builder = Agent::builder()
            .with_model(model.clone())
            .with_agent_event_tx(event_tx)
            .with_path(agent_path.clone())
            .with_config_json(config_json)
            .with_system_prompt_identity(&profile.agent_identity)
            .with_storage(storage.clone());

        if let Some(ref prompt) = profile.system_prompt {
            builder = builder.with_system_prompt_section(prompt);
        } else {
            builder = builder.with_system_prompt_section(crate::config::default_system_prompt());
        }

        builder = builder
            .with_tools(tool_list)
            .with_cancel_token(cancel_token.clone());

        if let Ok(Some(record)) = storage.get_agent_by_name(agent_path.name()).await {
            tracing::info!(
                agent = %agent_path,
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
        let internal_tx = agent.internal_event_tx();
        let model_handle = agent.model_handle().clone();

        let agent_task = self.runtime_handle.spawn(async move {
            agent.run().await;
        });

        Ok(AgentHandle {
            agent_id,
            path: agent_path.clone(),
            internal_tx,
            event_rx,
            agent_task,
            cancel_token,
            model: model_handle,
        })
    }

    /// Assemble the tool set for a profile, respecting its feature flags.
    async fn tools_from_profile(
        &self,
        agent_path: &agentik_types::AgentPath,
        profile: &agentik_core::AgentProfile,
    ) -> HostResult<Vec<agentik_core::tools::ToolRegistration>> {
        use crate::tools::*;
        use agentik_core::tools::ToolRegistration;

        let file_storage = self.file_storage.clone();
        let datalake = self.datalake.clone();
        let engine_client = self.engine_manager.client_for_session(&profile.name);

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
            let bib_shared = self.bib.clone();
            let bib_tools = bib_base::bib_all_registrations(
                bib_shared.bib.clone(),
                bib_shared.gateway.clone(),
                Some(bib_shared.europe_pmc.clone()),
            );
            tools.extend(bib_tools);
        }

        if profile.enable_writing {
            let ws = self.writing.clone();
            let writing_tools = writing_base::writing_all_registrations(
                ws.store.clone(),
                ws.bib.as_ref().map(|b| b.bib.clone()),
                ws.engine.clone(),
            );
            tools.extend(writing_tools);
        }

        // Host control tools (spawn_agent, delegate_to, list_agents, etc.)
        // Pass the agent's own path so list_agents / route_task can exclude self.
        tools.extend(crate::host_tools::host_tools(
            self.host_control.clone(),
            agent_path,
        ));

        Ok(tools)
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
    pub path: agentik_types::AgentPath,
    internal_tx: tokio::sync::mpsc::UnboundedSender<InternalEvent>,
    event_rx: tokio::sync::mpsc::UnboundedReceiver<AgentEvent>,
    agent_task: tokio::task::JoinHandle<()>,
    cancel_token: CancellationToken,
    model: Arc<ArcSwapOption<Model>>,
}

impl AgentHandle {
    /// Convenience: short name (last path segment).
    pub fn name(&self) -> &str {
        self.path.name()
    }
}

impl AgentHandle {
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

    /// Gracefully signal the agent to shut down.
    ///
    /// Sends `InternalEvent::Shutdown` — the agent's `run()` loop will
    /// process it, pause all sessions (persisting snapshots + ending WAL
    /// sessions), and then exit. The caller should subsequently await
    /// [`join`](Self::join) to ensure the task has fully completed before
    /// the runtime is dropped.
    pub fn shutdown(&mut self) {
        let _ = self.internal_tx.send(InternalEvent::Shutdown);
    }

    /// Consume and await the background agent task. Call this after
    /// [`shutdown`](Self::shutdown) to ensure all sessions are paused and
    /// snapshots are persisted before the runtime is torn down.
    pub async fn join(self) {
        let _ = self.agent_task.await;
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

    // ── Session management ────────────────────────────────

    /// Create a new session within this agent. Returns the new session ID.
    ///
    /// If `fork_from` is `Some(parent_id)`, the new session deep-clones the
    /// parent's memory (conversation branching). Otherwise the session
    /// starts with an empty conversation.
    pub fn create_session(
        &self,
        title: Option<String>,
        fork_from: Option<uuid::Uuid>,
    ) -> uuid::Uuid {
        let id = uuid::Uuid::new_v4();
        let _ = self.internal_tx.send(InternalEvent::CreateSession {
            id,
            fork_from,
            title,
        });
        id
    }

    /// Switch the active session to `id`. Pauses the current session and
    /// activates the target.
    pub fn switch_session(&self, id: uuid::Uuid) {
        let _ = self.internal_tx.send(InternalEvent::SwitchSession { id });
    }

    /// Close and remove a session.
    pub fn close_session(&self, id: uuid::Uuid) {
        let _ = self.internal_tx.send(InternalEvent::CloseSession { id });
    }

    /// Request a list of all sessions. The reply arrives as
    /// `AgentEvent::SessionList` on the event channel.
    pub fn list_sessions(&self) {
        let _ = self.internal_tx.send(InternalEvent::ListSessions);
    }

    /// Rename a session.
    pub fn rename_session(&self, id: uuid::Uuid, title: String) {
        let _ = self
            .internal_tx
            .send(InternalEvent::RenameSession { id, title });
    }
}

// ═══════════════════════════════════════════════════════════════════════
// RuntimeHost — top-level multi-agent manager
// ═══════════════════════════════════════════════════════════════════════

/// Top-level runtime that owns shared infrastructure, a persistent
/// [`AgentNetwork`] for topology routing, and an agent registry for
/// multiplexed event access.
///
/// Created once per process via [`RuntimeHost::open`]. Individual agents
/// are spawned via [`RuntimeHost::spawn_agent`]. For multi-agent topologies,
/// use the topology control API ([`add_node`](Self::add_node),
/// [`connect`](Self::connect), etc.) combined with
/// [`recv_any`](Self::recv_any) for unified event polling.
///
/// **Not `Clone`** — owns the agent network and registry. Pass `&self` or
/// `&mut self` to consumers rather than cloning.
pub struct RuntimeHost {
    infra: SharedInfra,
    /// Persistent topology routing engine — same lifetime as the host.
    network: AgentNetwork,
    /// Per-agent relay entries for multiplexed event access.
    agents: HashMap<String, AgentEntry>,
    /// Multiplexed event channel — each relay task pushes here.
    event_tx: UnboundedSender<TaggedEvent>,
    /// Receiver half (owned, drained via `recv_any`).
    event_rx: UnboundedReceiver<TaggedEvent>,
    /// Command channel from agent tools (HostControl).
    cmd_rx: tokio::sync::mpsc::UnboundedReceiver<crate::control::HostCommand>,
    /// Clonable control handle — passed to agent tools.
    control: crate::control::HostControl,
    /// Pending tool-delegation reply channels: maps delegatee name → reply.
    /// When the delegatee Dones, its response is sent through the channel.
    tool_delegations: HashMap<String, tokio::sync::oneshot::Sender<String>>,
    /// Cached profiles (blueprints) loaded at startup. Used by GetStatus
    /// and route_task so agents can discover what they can spawn.
    profiles: Vec<agentik_core::AgentProfile>,
    /// Current model (for spawning agents from tools). Set by TUI at startup.
    model: Option<Arc<ArcSwapOption<Model>>>,
    /// Channel for receiving spawned agent handles from background tasks.
    /// Background spawn tasks send (handle, info) here; the main loop
    /// drains and registers them.
    registration_rx: tokio::sync::mpsc::UnboundedReceiver<(AgentHandle, crate::control::AgentInfo)>,
    registration_tx: tokio::sync::mpsc::UnboundedSender<(AgentHandle, crate::control::AgentInfo)>,
    /// Notification channel — fires when an agent is registered or
    /// shut down. The TUI subscribes to this to keep its session list
    /// in sync with RuntimeHost's agent registry.
    notify_tx: tokio::sync::mpsc::UnboundedSender<HostEvent>,
    notify_rx: tokio::sync::mpsc::UnboundedReceiver<HostEvent>,
}

/// Lifecycle events emitted by RuntimeHost. The TUI subscribes to keep
/// its session tabs in sync with backend agent state.
#[derive(Debug, Clone)]
pub enum HostEvent {
    /// An agent was just registered with the host's relay.
    AgentRegistered {
        path: agentik_types::AgentPath,
        info: crate::control::AgentInfo,
    },
    /// An agent was shut down and removed from the registry.
    AgentUnregistered { path: String },
}

/// An `AgentEvent` tagged with the agent name that produced it.
pub type TaggedEvent = (String, AgentEvent);

/// Commands sent to a per-agent relay task.
enum AgentCommand {
    Message(String),
    Shutdown,
    Cancel,
    ListSessions,
    CreateSession {
        title: Option<String>,
        fork_from: Option<uuid::Uuid>,
    },
    SwitchSession(uuid::Uuid),
    CloseSession(uuid::Uuid),
    RenameSession {
        id: uuid::Uuid,
        title: String,
    },
    SetModel(Model),
}

/// Internal entry for one registered agent.
struct AgentEntry {
    cmd_tx: UnboundedSender<AgentCommand>,
    _relay_task: JoinHandle<()>,
    /// Full hierarchical path — source of truth for identity.
    /// Mirrors the HashMap key but kept here for typed access within entries.
    #[allow(dead_code)]
    path: agentik_types::AgentPath,
    /// Capability metadata for routing and discovery.
    info: crate::control::AgentInfo,
    /// Shared model slot — same Arc as the relay's AgentHandle.
    /// Allows querying and hot-swapping the model without direct
    /// access to the moved AgentHandle.
    model: Arc<ArcSwapOption<Model>>,
}

impl RuntimeHost {
    /// Open shared infrastructure and create an empty agent network.
    pub async fn open(config: &RuntimeConfig) -> HostResult<Self> {
        let mut infra = SharedInfra::open(config).await?;
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
        let (registration_tx, registration_rx) = mpsc::unbounded_channel();
        let (notify_tx, notify_rx) = mpsc::unbounded_channel();
        let control = crate::control::HostControl::new(cmd_tx);
        infra.host_control = Some(control.clone());
        Ok(Self {
            infra,
            network: AgentNetwork::new(),
            agents: HashMap::new(),
            event_tx,
            event_rx,
            cmd_rx,
            control,
            tool_delegations: HashMap::new(),
            profiles: Vec::new(),
            model: None,
            registration_rx,
            registration_tx,
            notify_tx,
            notify_rx,
        })
    }

    /// Returns a clonable [`HostControl`] for passing to agent tools.
    /// Agents use this to spawn peers, manage topology, send messages,
    /// and query system status.
    pub fn control(&self) -> crate::control::HostControl {
        self.control.clone()
    }

    /// Set cached profiles (blueprints). Called by the TUI after loading
    /// profiles from storage at startup. Enables agents to discover
    /// what profiles they can spawn via `list_agents` / `route_task`.
    pub fn set_profiles(&mut self, profiles: Vec<agentik_core::AgentProfile>) {
        self.profiles = profiles;
    }

    /// Set the current model (for spawning agents from tools).
    pub fn set_model(&mut self, model: Arc<ArcSwapOption<Model>>) {
        self.model = Some(model);
    }

    /// Drain and execute all pending commands from agent tools.
    /// Call this in the event loop (e.g. at each render tick).
    pub fn try_process_commands(&mut self) {
        while let Ok(cmd) = self.cmd_rx.try_recv() {
            self.process_command(cmd);
        }
    }

    /// Await either a host command or a background spawn completion.
    /// Event-driven — wakes when either arrives. Use as a `select!`
    /// branch in the event loop.
    pub async fn recv_and_process_command(&mut self) {
        tokio::select! {
            cmd = self.cmd_rx.recv() => {
                if let Some(cmd) = cmd {
                    self.process_command(cmd);
                }
            }
            reg = self.registration_rx.recv() => {
                if let Some((handle, info)) = reg {
                    let path = handle.path.clone();
                    let info_clone = info.clone();
                    self.register_agent(handle, info);
                    let _ = self.notify_tx.send(HostEvent::AgentRegistered {
                        path: path.clone(),
                        info: info_clone,
                    });
                    tracing::info!(agent = %path, "background spawn completed and registered");
                }
            }
        }
    }

    fn process_command(&mut self, cmd: crate::control::HostCommand) {
        use crate::control::{HostCommand, HostStatus};
        match cmd {
            HostCommand::Spawn {
                name,
                caller_path,
                profile_name,
                reply_tx,
            } => {
                // Derive child path from caller's path + the LLM-provided segment.
                let child_path = match caller_path.join(&name) {
                    Ok(p) => p,
                    Err(e) => {
                        let _ = reply_tx.send(Err(format!("invalid agent name `{name}`: {e}")));
                        return;
                    }
                };
                // Reject duplicate paths.
                if self.agents.contains_key(child_path.as_str()) {
                    let _ =
                        reply_tx.send(Err(format!("agent at path `{child_path}` already exists")));
                    return;
                }
                // Look up profile from cache.
                let Some(profile) = self
                    .profiles
                    .iter()
                    .find(|p| p.name == profile_name)
                    .cloned()
                else {
                    let _ = reply_tx.send(Err(format!(
                        "Profile '{profile_name}' not found. Available: {}",
                        self.profiles
                            .iter()
                            .map(|p| p.name.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    )));
                    return;
                };
                // Need a model to spawn.
                let Some(ref model) = self.model else {
                    let _ = reply_tx.send(Err("No model configured on host.".into()));
                    return;
                };

                // Spawn in background — agent creation is async.
                let infra = self.infra.clone();
                let model = model.clone();
                let reg_tx = self.registration_tx.clone();
                let info =
                    capability_from_profile(child_path.name(), child_path.as_str(), &profile);
                let path_for_spawn = child_path.clone();

                self.infra.runtime_handle.spawn(async move {
                    match infra
                        .spawn_agent(&path_for_spawn, &profile, model, None)
                        .await
                    {
                        Ok(handle) => {
                            let registered_path = handle.path.as_str().to_string();
                            let _ = reply_tx.send(Ok(registered_path));
                            let _ = reg_tx.send((handle, info));
                        }
                        Err(e) => {
                            let _ = reply_tx.send(Err(e.to_string()));
                        }
                    }
                });
            }
            HostCommand::SpawnWithProfile {
                name,
                caller_path,
                profile,
                model_override,
                reply_tx,
            } => {
                // Derive child path from caller's path + the LLM-provided segment.
                let child_path = match caller_path.join(&name) {
                    Ok(p) => p,
                    Err(e) => {
                        let _ = reply_tx.send(Err(format!("invalid agent name `{name}`: {e}")));
                        return;
                    }
                };
                // Reject duplicate paths.
                if self.agents.contains_key(child_path.as_str()) {
                    let _ =
                        reply_tx.send(Err(format!("agent at path `{child_path}` already exists")));
                    return;
                }
                // Resolve model: use override if provided, else global model.
                let model = match (model_override, self.model.as_ref()) {
                    (Some(m), _) => Arc::new(ArcSwapOption::from_pointee(Some(m))),
                    (None, Some(g)) => g.clone(),
                    (None, None) => {
                        let _ = reply_tx.send(Err("No model configured on host.".into()));
                        return;
                    }
                };

                let infra = self.infra.clone();
                let reg_tx = self.registration_tx.clone();
                let info =
                    capability_from_profile(child_path.name(), child_path.as_str(), &profile);
                let path_for_spawn = child_path.clone();

                self.infra.runtime_handle.spawn(async move {
                    match infra
                        .spawn_agent(&path_for_spawn, &profile, model, None)
                        .await
                    {
                        Ok(handle) => {
                            let registered_path = handle.path.as_str().to_string();
                            let _ = reply_tx.send(Ok(registered_path));
                            let _ = reg_tx.send((handle, info));
                        }
                        Err(e) => {
                            let _ = reply_tx.send(Err(e.to_string()));
                        }
                    }
                });
            }
            HostCommand::Shutdown { name } => {
                let resolved = self.resolve_agent(&name).unwrap_or(name);
                self.shutdown_agent(&resolved);
            }
            HostCommand::AddNode {
                name,
                profile,
                initial_prompt,
            } => {
                let _ = self.network.add_node(agentik_network::NodeSpec {
                    name,
                    profile,
                    initial_prompt,
                });
            }
            HostCommand::RemoveNode { name } => {
                self.network.remove_node(&name);
            }
            HostCommand::Connect { from, to, trigger } => {
                let _ = self.network.connect(&from, &to, trigger, None);
            }
            HostCommand::Disconnect { from, to } => {
                self.network.disconnect(&from, &to);
            }
            HostCommand::DeliverMessage { name, message } => {
                let resolved = self.resolve_agent(&name).unwrap_or(name);
                self.send_to(&resolved, message);
            }
            HostCommand::Delegate {
                to,
                message,
                reply_tx,
            } => {
                // Resolve target: full path or short name.
                let resolved = match self.resolve_agent(&to) {
                    Some(path) => path,
                    None => {
                        let _ = reply_tx.send(format!(
                            "Error: agent '{to}' is not registered. \
                             Use list_agents to see available agents, \
                             or spawn_agent to create one first."
                        ));
                        return;
                    }
                };
                // Record the reply channel — when `to` Dones, its response
                // is sent through reply_tx (handled in step()).
                self.tool_delegations.insert(resolved.clone(), reply_tx);
                self.send_to(&resolved, message);
            }
            HostCommand::GetStatus { reply_tx } => {
                let g = self.network.graph();
                let status = HostStatus {
                    agents: self.agents.values().map(|e| e.info.clone()).collect(),
                    profiles: self
                        .profiles
                        .iter()
                        .map(|p| capability_from_profile(&p.name, &p.name, p))
                        .collect(),
                    nodes: g.node_names().into_iter().map(String::from).collect(),
                    edge_count: g.edge_count(),
                    is_cyclic: g.is_cyclic(),
                    roots: g.root_nodes().into_iter().map(String::from).collect(),
                    leaves: g.leaf_nodes().into_iter().map(String::from).collect(),
                    rounds: self.network.rounds(),
                    is_finished: self.network.is_finished(),
                    termination: format!("{:?}", self.network.termination()),
                };
                let _ = reply_tx.send(status);
            }
            HostCommand::SetTermination { spec } => {
                self.network.set_termination(spec);
            }
            HostCommand::ResetRunState => {
                self.network.reset_run_state();
            }
            HostCommand::InjectPrompts => {
                self.inject_initial_prompts();
            }
            HostCommand::RouteTask {
                description,
                exclude,
                reply_tx,
            } => {
                let result = self.route_task(&description, exclude.as_deref());
                let _ = reply_tx.send(result);
            }
            HostCommand::GetAgentInfo { name, reply_tx } => {
                // Resolve agent name, then check running agents first,
                // fall back to profiles (by short name).
                let resolved = self.resolve_agent(&name);
                let info = resolved
                    .as_ref()
                    .and_then(|key| self.agents.get(key).map(|e| e.info.clone()))
                    .or_else(|| {
                        self.profiles
                            .iter()
                            .find(|p| p.name == name)
                            .map(|p| capability_from_profile(&p.name, &p.name, p))
                    });
                let _ = reply_tx.send(info);
            }

            // ── Session management (forwarded to relay) ──
            HostCommand::CancelAgent { name } => {
                self.send_agent_command(&name, AgentCommand::Cancel);
            }
            HostCommand::ListSessions { name } => {
                self.send_agent_command(&name, AgentCommand::ListSessions);
            }
            HostCommand::CreateSession {
                name,
                title,
                fork_from,
            } => {
                self.send_agent_command(&name, AgentCommand::CreateSession { title, fork_from });
            }
            HostCommand::SwitchSession { name, session_id } => {
                self.send_agent_command(&name, AgentCommand::SwitchSession(session_id));
            }
            HostCommand::CloseSession { name, session_id } => {
                self.send_agent_command(&name, AgentCommand::CloseSession(session_id));
            }
            HostCommand::RenameSession {
                name,
                session_id,
                title,
            } => {
                self.send_agent_command(
                    &name,
                    AgentCommand::RenameSession {
                        id: session_id,
                        title,
                    },
                );
            }

            // ── Model management ──
            HostCommand::SetAgentModel { name, model } => {
                self.send_agent_command(&name, AgentCommand::SetModel(model));
            }
            HostCommand::GetAgentModel { name, reply_tx } => {
                let resolved = self.resolve_agent(&name);
                let info = resolved
                    .as_ref()
                    .and_then(|key| self.agents.get(key))
                    .and_then(|e| {
                        e.model
                            .load_full()
                            .as_deref()
                            .map(|m| (m.model_info.model_name.clone(), m.model_info.context_length))
                    });
                let _ = reply_tx.send(info);
            }
        }
    }

    /// Resolve an agent reference to its full path key in the registry.
    ///
    /// Accepts:
    /// 1. **Full path** (`/root/researcher/worker`) — exact match.
    /// 2. **Short name** (`researcher`) — matches when unambiguous.
    ///
    /// Returns the HashMap key string, or `None` if no match / ambiguous.
    fn resolve_agent(&self, target: &str) -> Option<String> {
        // 1. Exact full-path match.
        if target.starts_with("/root") && self.agents.contains_key(target) {
            return Some(target.to_string());
        }
        // 2. Short-name match: collect all agents whose last path segment
        //    equals `target`.
        let matches: Vec<&String> = self
            .agents
            .keys()
            .filter(|key| key.rsplit('/').next().unwrap_or(key) == target)
            .collect();
        match matches.len() {
            1 => Some(matches[0].clone()),
            0 => None,
            _ => {
                tracing::warn!(
                    target = %target,
                    candidates = ?matches,
                    "ambiguous agent name — use full path to disambiguate"
                );
                None
            }
        }
    }

    /// Forward a command to a named agent's relay task.
    /// `name` is resolved via [`resolve_agent`](Self::resolve_agent).
    fn send_agent_command(&self, name: &str, cmd: AgentCommand) {
        if let Some(resolved) = self.resolve_agent(name) {
            if let Some(entry) = self.agents.get(&resolved) {
                let _ = entry.cmd_tx.send(cmd);
                return;
            }
        }
        tracing::warn!(agent = %name, "send_agent_command: agent not registered");
    }

    /// Notify listeners that an agent has been shut down.
    /// Called from `shutdown_agent` / `shutdown_all_agents`.
    fn notify_unregistered(&self, name: &str) {
        let _ = self.notify_tx.send(HostEvent::AgentUnregistered {
            path: name.to_string(),
        });
    }

    /// Route a task description to the best-matching agent.
    /// Considers both running agents and available profiles (blueprints).
    /// Running agents get a small bonus score since they're immediately
    /// available for delegation.
    fn route_task(&self, description: &str, exclude: Option<&str>) -> crate::control::RouteResult {
        let desc_lower = description.to_lowercase();
        let desc_words: std::collections::HashSet<&str> = desc_lower
            .split_whitespace()
            .filter(|w| w.len() > 2)
            .collect();

        let score_info = |info: &crate::control::AgentInfo, is_running: bool| {
            let matched_tags: Vec<String> = info
                .tags
                .iter()
                .filter(|tag| desc_lower.contains(tag.as_str()))
                .cloned()
                .collect();

            let tag_score = matched_tags.len() as f64 * 3.0;

            let info_text = format!(
                "{} {}",
                info.summary.to_lowercase(),
                info.expertise.join(" ").to_lowercase()
            );
            let info_words: std::collections::HashSet<&str> = info_text
                .split_whitespace()
                .filter(|w| w.len() > 2)
                .collect();

            let word_overlap = desc_words.intersection(&info_words).count() as f64;
            let running_bonus = if is_running { 0.5 } else { 0.0 };
            let score = tag_score + word_overlap + running_bonus;

            crate::control::RouteCandidate {
                agent: info.name.clone(),
                score,
                matched_tags,
            }
        };

        // Score running agents (excluding the caller itself).
        let mut candidates: Vec<crate::control::RouteCandidate> = self
            .agents
            .values()
            .filter(|e| Some(e.info.name.as_str()) != exclude)
            .map(|e| score_info(&e.info, true))
            .collect();

        // Score profiles (excluding those already running under the same name
        // and the caller itself).
        let running_names: std::collections::HashSet<&str> =
            self.agents.keys().map(|s| s.as_str()).collect();
        candidates.extend(
            self.profiles
                .iter()
                .filter(|p| !running_names.contains(p.name.as_str()))
                .filter(|p| Some(p.name.as_str()) != exclude)
                .map(|p| score_info(&capability_from_profile(&p.name, &p.name, p), false)),
        );

        let mut candidates: Vec<_> = candidates.into_iter().filter(|c| c.score > 0.0).collect();

        candidates.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        let (best, reason) = if let Some(top) = candidates.first() {
            let reason = if top.matched_tags.is_empty() {
                format!("Best keyword match (score: {:.1})", top.score)
            } else {
                format!(
                    "Matched tags: {} (score: {:.1})",
                    top.matched_tags.join(", "),
                    top.score
                )
            };
            (top.agent.clone(), reason)
        } else {
            ("none".into(), "No matching agent found.".into())
        };

        crate::control::RouteResult {
            agent: best,
            reason,
            score: candidates.first().map(|c| c.score).unwrap_or(0.0),
            candidates,
        }
    }

    /// Returns a reference to the shared storage.
    pub fn storage(&self) -> &Arc<dyn AgentStorage> {
        &self.infra.storage
    }

    /// Returns a clone of the shared infra (for advanced use).
    pub fn infra(&self) -> SharedInfra {
        self.infra.clone()
    }

    /// Returns a reference to the persistent topology network.
    pub fn network(&self) -> &AgentNetwork {
        &self.network
    }

    /// Returns a mutable reference to the topology network.
    pub fn network_mut(&mut self) -> &mut AgentNetwork {
        &mut self.network
    }

    // ── Topology control ───────────────────────────────────

    /// Add a node to the topology.
    pub fn add_node(&mut self, name: &str, profile: &str) -> Result<(), String> {
        self.network.add_node(NodeSpec {
            name: name.into(),
            profile: profile.into(),
            initial_prompt: None,
        })
    }

    /// Add a node with an initial prompt.
    pub fn add_node_with_prompt(
        &mut self,
        name: &str,
        profile: &str,
        prompt: impl Into<String>,
    ) -> Result<(), String> {
        self.network.add_node(NodeSpec {
            name: name.into(),
            profile: profile.into(),
            initial_prompt: Some(prompt.into()),
        })
    }

    /// Remove a node from the topology (and clean up routing state).
    pub fn remove_node(&mut self, name: &str) {
        self.network.remove_node(name);
    }

    /// Connect two nodes with a trigger (request-response delegation).
    pub fn connect(&mut self, from: &str, to: &str, trigger: EdgeTrigger) -> Result<(), String> {
        self.network.connect(from, to, trigger, None)
    }

    /// Remove all edges between two nodes.
    pub fn disconnect(&mut self, from: &str, to: &str) -> usize {
        self.network.disconnect(from, to)
    }

    /// Set the termination condition.
    pub fn set_termination(&mut self, termination: TerminationSpec) {
        self.network.set_termination(termination);
    }

    /// Reset routing state (buffers, counts, finished) while keeping the
    /// topology graph intact.
    pub fn reset_run_state(&mut self) {
        self.network.reset_run_state();
    }

    // ── Agent registration + multiplexed transport ─────────

    /// Register an [`AgentHandle`] with the host's internal multiplexer.
    ///
    /// After registration, use [`send_to`](Self::send_to) for message
    /// delivery and [`recv_any`](Self::recv_any) for unified event polling.
    /// The host takes ownership of the handle — do not use it directly
    /// after registration.
    pub fn register_agent(&mut self, handle: AgentHandle, info: crate::control::AgentInfo) {
        let path = handle.path.clone();
        let relay_name = path.as_str().to_string();
        let model = handle.model.clone(); // Clone Arc before moving handle
        let event_tx = self.event_tx.clone();
        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel::<AgentCommand>();

        let relay_task = self.infra.runtime_handle.spawn(async move {
            relay_loop(handle, cmd_rx, event_tx, relay_name).await;
        });

        self.agents.insert(
            path.as_str().to_string(),
            AgentEntry {
                cmd_tx,
                _relay_task: relay_task,
                path,
                info,
                model,
            },
        );
    }

    /// Spawn an agent and immediately register it with the host's
    /// multiplexer. Returns the agent name.
    pub async fn spawn_and_register(
        &mut self,
        agent_name: &str,
        profile: &agentik_core::AgentProfile,
        global_model: Arc<ArcSwapOption<Model>>,
        model_override: Option<Model>,
    ) -> HostResult<String> {
        let path = agentik_types::AgentPath::root()
            .join(agent_name)
            .map_err(|e| HostError::Other(e.to_string()))?;
        let handle = self
            .spawn_agent(&path, profile, global_model, model_override)
            .await?;
        let name = handle.path.as_str().to_string();
        let info = capability_from_profile(handle.path.name(), handle.path.as_str(), profile);
        self.register_agent(handle, info);
        Ok(name)
    }

    /// Send a message to a named agent (via the relay task).
    ///
    /// `name` should already be a resolved full path. Callers that receive
    /// user/LLM-provided names should call [`resolve_agent`](Self::resolve_agent)
    /// first.
    pub fn send_to(&self, name: &str, message: String) {
        if let Some(entry) = self.agents.get(name) {
            let _ = entry.cmd_tx.send(AgentCommand::Message(message));
        }
    }

    /// Query the model info for a named agent (for TUI rendering).
    pub fn agent_model_info(&self, name: &str) -> Option<(String, u64)> {
        self.agents.get(name).and_then(|e| {
            e.model
                .load_full()
                .as_deref()
                .map(|m| (m.model_info.model_name.clone(), m.model_info.context_length))
        })
    }

    /// Inject initial prompts for all nodes that have them.
    pub fn inject_initial_prompts(&mut self) {
        let messages = self.network.initial_messages();
        for (node, prompt) in messages {
            self.send_to(&node, prompt);
        }
    }

    /// Shut down a named agent and remove it from the registry.
    pub fn shutdown_agent(&mut self, name: &str) {
        if let Some(entry) = self.agents.remove(name) {
            let _ = entry.cmd_tx.send(AgentCommand::Shutdown);
        }
        self.notify_unregistered(name);
    }

    /// Shut down all registered agents and await their graceful exit.
    ///
    /// Each agent's relay task calls `handle.join()` which waits for the
    /// agent's `run()` loop to process the `Shutdown` event, pause all
    /// sessions (persist snapshots + end WAL sessions), and flush pending
    /// `persist_worker` operations.
    ///
    /// **Must be called from within the tokio runtime** (e.g. inside a
    /// `block_on`) so that background tasks can make progress while we
    /// await their completion.
    pub async fn shutdown_all_agents_and_wait(&mut self) {
        let names: Vec<String> = self.agents.keys().cloned().collect();
        let mut relay_tasks = Vec::new();
        for (_, entry) in self.agents.drain() {
            let _ = entry.cmd_tx.send(AgentCommand::Shutdown);
            relay_tasks.push(entry._relay_task);
        }
        for name in names {
            self.notify_unregistered(&name);
        }
        // Wait for all relay tasks to finish. Each relay loop calls
        // `handle.join().await` before exiting, which in turn waits for
        // the agent's `run()` to complete its session-pause shutdown.
        for task in relay_tasks {
            let _ = task.await;
        }
    }

    /// Shut down all registered agents (fire-and-forget).
    ///
    /// **Prefer [`shutdown_all_agents_and_wait`](Self::shutdown_all_agents_and_wait)
    /// when the runtime is still alive** — this method does NOT wait for
    /// agents to gracefully pause their sessions. It exists for scenarios
    /// where the caller cannot await (e.g. sync code paths).
    pub fn shutdown_all_agents(&mut self) {
        let names: Vec<String> = self.agents.keys().cloned().collect();
        for (_, entry) in self.agents.drain() {
            let _ = entry.cmd_tx.send(AgentCommand::Shutdown);
        }
        for name in names {
            self.notify_unregistered(&name);
        }
    }

    /// Receive the next event from any registered agent.
    ///
    /// This also handles delegation plumbing:
    /// - `LlmResponse` events are accumulated in the network's response
    ///   buffer (so `accumulated_response` works when Done arrives).
    /// - `Done` events trigger the tool delegation reply: the accumulated
    ///   response is sent to the waiting `delegate_to` tool via its
    ///   `reply_tx` channel.
    ///
    /// The raw event is still returned to the caller for UI rendering.
    pub async fn recv_any(&mut self) -> Option<TaggedEvent> {
        let (name, event) = self.event_rx.recv().await?;

        // For Done events: capture accumulated response BEFORE process_event
        // (process_event drains the response buffer).
        let delegation_response = if matches!(event, AgentEvent::Done) {
            if self.tool_delegations.contains_key(&name) {
                Some(self.network.accumulated_response(&name).to_string())
            } else {
                None
            }
        } else {
            None
        };

        // Feed the event through the network (accumulates LlmResponse,
        // handles topology-edge delegation routing, termination checks).
        let actions = self.network.process_event(&name, &event);

        // If we captured a delegation response, send it to the waiting tool.
        if let Some(response) = delegation_response {
            if let Some(reply_tx) = self.tool_delegations.remove(&name) {
                let _ = reply_tx.send(response);
                tracing::info!(agent = %name, "delegation response delivered to caller tool");
            }
        }

        // Execute any routing actions (topology-edge based forwarding).
        for action in &actions {
            if let agentik_network::RoutingAction::Send { to, message } = action {
                self.send_to(to, message.clone());
            }
        }

        Some((name, event))
    }

    /// Receive the next host lifecycle event (agent registered / unregistered).
    pub async fn recv_event(&mut self) -> Option<HostEvent> {
        self.notify_rx.recv().await
    }

    /// Try to receive a host lifecycle event without blocking.
    pub fn try_recv_event(&mut self) -> Option<HostEvent> {
        self.notify_rx.try_recv().ok()
    }

    /// Try to receive an event without blocking.
    pub fn try_recv_any(&mut self) -> Option<TaggedEvent> {
        self.event_rx.try_recv().ok()
    }

    /// One iteration of the event loop: receive an event, process it
    /// through the topology network, and execute routing actions.
    ///
    /// Returns `(agent_name, raw_event, routing_actions)` or `None` if
    /// all agents are done.
    ///
    /// **Note**: delegation plumbing (response accumulation + tool reply)
    /// is handled inside `recv_any()`. This method is kept for callers
    /// that need the routing actions.
    pub async fn step(&mut self) -> Option<(String, AgentEvent)> {
        // Drain any pending tool commands first.
        self.try_process_commands();
        self.recv_any().await
    }

    /// Check if a named agent is registered.
    pub fn contains_agent(&self, name: &str) -> bool {
        self.agents.contains_key(name)
    }

    /// Number of registered agents.
    pub fn agent_count(&self) -> usize {
        self.agents.len()
    }

    /// Names of all registered agents.
    pub fn agent_names(&self) -> Vec<&str> {
        self.agents.keys().map(|s| s.as_str()).collect()
    }

    /// Spawn a new agent from an [`AgentProfile`](agentik_core::AgentProfile).
    ///
    /// Delegates to [`SharedInfra::spawn_agent`] — which only needs the
    /// shared infrastructure, not the topology network or registry.
    /// This allows callers to clone [`SharedInfra`] (cheap, all `Arc`)
    /// into async tasks without cloning the non-`Clone` [`RuntimeHost`].
    pub async fn spawn_agent(
        &self,
        agent_path: &agentik_types::AgentPath,
        profile: &agentik_core::AgentProfile,
        global_model: Arc<ArcSwapOption<Model>>,
        model_override: Option<Model>,
    ) -> HostResult<AgentHandle> {
        self.infra
            .spawn_agent(agent_path, profile, global_model, model_override)
            .await
    }

    /// Returns a clonable [`SharedInfra`] that can spawn agents from
    /// within async tasks. Use this instead of cloning [`RuntimeHost`]
    /// (which is not `Clone`).
    ///
    /// ```ignore
    /// let infra = host.spawner();
    /// tokio::spawn(async move {
    ///     infra.spawn_agent(...).await
    /// });
    /// ```
    pub fn spawner(&self) -> SharedInfra {
        self.infra.clone()
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Capability extraction
// ═══════════════════════════════════════════════════════════════════════

/// Build [`AgentInfo`] from an [`AgentProfile`], auto-extracting tags,
/// expertise, and tool list from the profile's feature flags.
fn capability_from_profile(
    name: &str,
    path: &str,
    profile: &agentik_core::AgentProfile,
) -> crate::control::AgentInfo {
    let mut tags = Vec::new();
    let mut expertise = Vec::new();

    if profile.enable_bibliography {
        tags.push("literature".into());
        tags.push("bibliography".into());
        expertise.push("literature-search".into());
        expertise.push("citation-management".into());
    }
    if profile.enable_writing {
        tags.push("writing".into());
        tags.push("latex".into());
        tags.push("manuscript".into());
        expertise.push("document-editing".into());
        expertise.push("citation-insertion".into());
        expertise.push("latex-compilation".into());
    }
    if profile.enable_opengwas {
        tags.push("gwas".into());
        tags.push("genetics".into());
        expertise.push("gwas-analysis".into());
    }
    if profile.enable_opentargets {
        tags.push("drug-targets".into());
        expertise.push("target-identification".into());
    }
    if profile.enable_gwascatalog {
        tags.push("gwas-catalog".into());
        expertise.push("variant-lookup".into());
    }
    if profile.enable_iceberg {
        tags.push("datalake".into());
        tags.push("iceberg".into());
        expertise.push("data-lake-query".into());
    }
    if profile.enable_dag_history {
        tags.push("pipeline".into());
        expertise.push("dag-execution".into());
    }

    crate::control::AgentInfo {
        name: name.into(),
        path: path.into(),
        summary: profile.description.clone(),
        tags,
        expertise,
        tools: Vec::new(), // populated at runtime if needed
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Relay loop — per-agent event/command bridge
// ═══════════════════════════════════════════════════════════════════════

/// Owns one [`AgentHandle`] and bridges:
/// - **Inbound**: `AgentCommand`s from the host → `send_message`/`shutdown`
/// - **Outbound**: `AgentEvent`s from the agent → tagged events to host
async fn relay_loop(
    mut handle: AgentHandle,
    mut cmd_rx: UnboundedReceiver<AgentCommand>,
    event_tx: UnboundedSender<TaggedEvent>,
    name: String,
) {
    tracing::debug!(agent = %name, "relay task started");

    loop {
        tokio::select! {
            biased;

            event = handle.recv_event() => match event {
                Some(ev) => {
                    if event_tx.send((name.clone(), ev)).is_err() {
                        break;
                    }
                }
                None => break,
            },

            cmd = cmd_rx.recv() => match cmd {
                Some(AgentCommand::Message(text)) => {
                    handle.send_message(text);
                }
                Some(AgentCommand::Cancel) => {
                    handle.cancel();
                }
                Some(AgentCommand::ListSessions) => {
                    handle.list_sessions();
                }
                Some(AgentCommand::CreateSession { title, fork_from }) => {
                    let _ = handle.create_session(title, fork_from);
                }
                Some(AgentCommand::SwitchSession(id)) => {
                    handle.switch_session(id);
                }
                Some(AgentCommand::CloseSession(id)) => {
                    handle.close_session(id);
                }
                Some(AgentCommand::RenameSession { id, title }) => {
                    handle.rename_session(id, title);
                }
                Some(AgentCommand::SetModel(model)) => {
                    handle.set_model(model);
                }
                Some(AgentCommand::Shutdown) | None => {
                    handle.shutdown();
                    break;
                }
            }
        }
    }

    // After breaking out of the relay loop, wait for the agent's `run()`
    // task to finish. The agent needs time to process the Shutdown event,
    // pause all sessions (persist snapshots + end WAL sessions), and flush
    // any remaining persist_worker messages. Without this await, dropping
    // the runtime would kill the task mid-shutdown, causing data loss.
    handle.join().await;

    tracing::debug!(agent = %name, "relay task exited");
}

// ═══════════════════════════════════════════════════════════════════════
// Drop — best-effort shutdown
// ═══════════════════════════════════════════════════════════════════════

impl Drop for RuntimeHost {
    fn drop(&mut self) {
        for (_, entry) in self.agents.drain() {
            let _ = entry.cmd_tx.send(AgentCommand::Shutdown);
        }
    }
}
