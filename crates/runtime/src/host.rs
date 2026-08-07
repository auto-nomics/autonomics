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

        Ok(Self {
            engine_manager,
            file_storage,
            datalake,
            storage,
            runtime_handle: tokio::runtime::Handle::current(),
            bib: Arc::new(
                bib_base::BibShared::open_with(&config.bib_db_path, config.bib_http.clone())
                    .await?,
            ),
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
        agent_name: &str,
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

        let tool_list = self.tools_from_profile(profile).await?;

        let config_json = serde_json::to_value(profile).unwrap_or_default();
        let storage = self.storage.clone();

        let mut builder = Agent::builder()
            .with_model(model.clone())
            .with_agent_event_tx(event_tx)
            .with_name(agent_name)
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

        let agent_task = self.runtime_handle.spawn(async move {
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
    async fn tools_from_profile(
        &self,
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

        // Host control tools (spawn_agent, send_to_agent, connect_agents, etc.)
        tools.extend(crate::host_tools::host_tools(self.host_control.clone()));

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
}

/// An `AgentEvent` tagged with the agent name that produced it.
pub type TaggedEvent = (String, AgentEvent);

/// Commands sent to a per-agent relay task.
enum AgentCommand {
    Message(String),
    Shutdown,
}

/// Internal entry for one registered agent.
struct AgentEntry {
    cmd_tx: UnboundedSender<AgentCommand>,
    _relay_task: JoinHandle<()>,
}

impl RuntimeHost {
    /// Open shared infrastructure and create an empty agent network.
    pub async fn open(config: &RuntimeConfig) -> HostResult<Self> {
        let mut infra = SharedInfra::open(config).await?;
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
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
        })
    }

    /// Returns a clonable [`HostControl`] for passing to agent tools.
    /// Agents use this to spawn peers, manage topology, send messages,
    /// and query system status.
    pub fn control(&self) -> crate::control::HostControl {
        self.control.clone()
    }

    /// Drain and execute all pending commands from agent tools.
    /// Call this in the event loop (e.g. at each render tick).
    pub fn try_process_commands(&mut self) {
        use crate::control::HostCommand;
        while let Ok(cmd) = self.cmd_rx.try_recv() {
            self.process_command(cmd);
        }
    }

    fn process_command(&mut self, cmd: crate::control::HostCommand) {
        use crate::control::{HostCommand, HostStatus};
        match cmd {
            HostCommand::Spawn {
                name: _,
                profile_name,
                reply_tx,
            } => {
                // Look up profile by name. In the current design, profiles
                // are passed externally — we can't resolve them here.
                // Return an error indicating the caller should use
                // spawn_and_register directly, or we need a profile
                // registry on the host.
                let _ = reply_tx.send(Err(format!(
                    "Agent spawn via tool requires a profile registry. \
                     Profile '{profile_name}' cannot be resolved from within \
                     the host command loop. Use host.spawn_and_register() \
                     directly, or register profiles first."
                )));
            }
            HostCommand::Shutdown { name } => {
                self.shutdown_agent(&name);
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
            HostCommand::Connect {
                from,
                to,
                trigger,
            } => {
                let _ = self.network.connect(&from, &to, trigger, agentik_network::EdgeKind::Delegate, None);
            }
            HostCommand::Disconnect { from, to } => {
                self.network.disconnect(&from, &to);
            }
            HostCommand::SendTo { name, message } => {
                self.send_to(&name, message);
            }
            HostCommand::Delegate {
                to,
                message,
                reply_tx,
            } => {
                // Record the reply channel — when `to` Dones, its response
                // is sent through reply_tx (handled in step()).
                self.tool_delegations.insert(to.clone(), reply_tx);
                self.send_to(&to, message);
            }
            HostCommand::GetStatus { reply_tx } => {
                let g = self.network.graph();
                let status = HostStatus {
                    agents: self
                        .agents
                        .keys()
                        .cloned()
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

    /// Connect two nodes with a trigger (defaults to Delegate semantics).
    pub fn connect(&mut self, from: &str, to: &str, trigger: EdgeTrigger) -> Result<(), String> {
        self.network.connect(from, to, trigger, agentik_network::EdgeKind::Delegate, None)
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
    pub fn register_agent(&mut self, handle: AgentHandle) {
        let name = handle.name.clone();
        let relay_name = name.clone();
        let event_tx = self.event_tx.clone();
        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel::<AgentCommand>();

        let relay_task = self.infra.runtime_handle.spawn(async move {
            relay_loop(handle, cmd_rx, event_tx, relay_name).await;
        });

        self.agents.insert(
            name,
            AgentEntry {
                cmd_tx,
                _relay_task: relay_task,
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
        let handle = self
            .spawn_agent(agent_name, profile, global_model, model_override)
            .await?;
        let name = handle.name.clone();
        self.register_agent(handle);
        Ok(name)
    }

    /// Send a message to a named agent (via the relay task).
    pub fn send_to(&self, name: &str, message: String) {
        if let Some(entry) = self.agents.get(name) {
            let _ = entry.cmd_tx.send(AgentCommand::Message(message));
        }
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
    }

    /// Shut down all registered agents.
    pub fn shutdown_all_agents(&mut self) {
        for (_, entry) in self.agents.drain() {
            let _ = entry.cmd_tx.send(AgentCommand::Shutdown);
        }
    }

    /// Receive the next event from any registered agent.
    pub async fn recv_any(&mut self) -> Option<TaggedEvent> {
        self.event_rx.recv().await
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
    pub async fn step(&mut self) -> Option<(String, AgentEvent, Vec<RoutingAction>)> {
        // Drain any pending tool commands first.
        self.try_process_commands();

        let (name, event) = self.recv_any().await?;

        // If a Done event arrives and there's a pending tool delegation,
        // capture the accumulated response and send it to the waiting tool.
        // This must happen BEFORE process_event (which drains the buffer).
        if matches!(event, AgentEvent::Done) {
            if let Some(reply_tx) = self.tool_delegations.remove(&name) {
                let response = self.network.accumulated_response(&name).to_string();
                let _ = reply_tx.send(response);
            }
        }

        // Process the event through the topology network.
        let actions = self.network.process_event(&name, &event);
        for action in &actions {
            if let RoutingAction::Send { to, message } = action {
                self.send_to(to, message.clone());
            }
        }
        Some((name, event, actions))
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
        agent_name: &str,
        profile: &agentik_core::AgentProfile,
        global_model: Arc<ArcSwapOption<Model>>,
        model_override: Option<Model>,
    ) -> HostResult<AgentHandle> {
        self.infra
            .spawn_agent(agent_name, profile, global_model, model_override)
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
                Some(AgentCommand::Shutdown) | None => {
                    handle.shutdown();
                    break;
                }
            }
        }
    }

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
