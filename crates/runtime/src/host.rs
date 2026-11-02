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
//!
//! This module is the facade; the implementation lives in submodules:
//!
//! - [`shared_infra`] — [`SharedInfra`] and the per-agent spawn/tool assembly
//! - [`agent_handle`] — the per-agent control struct
//! - [`relay`] — agent registry entries, relay loop, message transport
//! - [`commands`] — `HostCommand` dispatch and name/hierarchy policy
//! - [`delegation`] — the delegation ledger
//! - [`persistence`] — durable projections of host state
//! - [`status`] — `AgentEvent → AgentLifecycleStatus` projection
//! - [`vfs_setup`] — VFS manifest, mounts, and catalog wiring
//! - [`bundles`] — the user data-bundle registry
//! - [`gc`] — background GC / distiller loops

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use agentik_core::storage::AgentStorage;
use agentik_network::AgentNetwork;
use agentik_sdk::model::Model;
use agentik_sdk::types::AgentEvent;
use arc_swap::ArcSwapOption;
use data_engine::runtime::DataEngineClient;
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};

use crate::config::RuntimeConfig;
use crate::control::DelegationStatus;
use crate::error::Result;

mod agent_handle;
mod bundles;
mod commands;
mod delegation;
mod events;
mod gc;
mod persistence;
mod relay;
mod shared_infra;
mod status;
mod vfs_setup;

pub use agent_handle::AgentHandle;
pub use events::{HostEvent, NextEvent, TaggedEvent};
pub use shared_infra::SharedInfra;
pub use vfs_setup::bibliography_file_storage;

use events::{PendingDelivery, SpawnNotice};

use delegation::{HostDelegation, push_delegation_progress};
use relay::AgentEntry;
use status::derive_agent_status;

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
    /// First-class delegation ledger. Keyed by stable delegation ID, so the
    /// same target can safely process multiple queued requests.
    delegations: HashMap<uuid::Uuid, HostDelegation>,
    /// Current model (for spawning agents from tools). Set by TUI at startup.
    model: Option<Arc<ArcSwapOption<Model>>>,
    /// Channel for receiving background spawn outcomes. Background spawn
    /// and virtual-actor activation tasks report here; the main loop
    /// registers successes and fails buffered deliveries on errors.
    registration_rx: tokio::sync::mpsc::UnboundedReceiver<SpawnNotice>,
    registration_tx: tokio::sync::mpsc::UnboundedSender<SpawnNotice>,
    /// Deliveries deferred until a dormant identity finishes activating
    /// (virtual-actor mode). Keyed by registry path; flushed by
    /// [`Self::register_background_spawn`] once the instance is live.
    activation_pending: HashMap<String, Vec<PendingDelivery>>,
    /// Resolves per-agent `preferred_model` specs during virtual-actor
    /// activation. Installed by the daemon (the model actor lives there);
    /// without it only preference-less identities can activate.
    model_resolver: Option<crate::model_actor::ModelHandle>,
    /// Virtual-actor idle passivation TTL (from the runtime config).
    /// `None` disables passivation — instances live until explicitly
    /// shut down.
    agent_idle_ttl: Option<std::time::Duration>,
    /// Notification channel — fires when an agent is registered or
    /// shut down. The TUI subscribes to this to keep its session list
    /// in sync with RuntimeHost's agent registry.
    notify_tx: tokio::sync::mpsc::UnboundedSender<HostEvent>,
    notify_rx: tokio::sync::mpsc::UnboundedReceiver<HostEvent>,
    /// Broadcast event channel — same `HostEvent` stream as `notify_tx`,
    /// but multi-consumer. Each subscriber gets its own `broadcast::Receiver`
    /// that lags independently. Used by event subscribers (e.g. the TUI)
    /// so multiple consumers can observe agent status changes concurrently
    /// without stealing events from each other.
    /// Buffer of 256 should be plenty for in-flight status transitions;
    /// if it overflows, subscribers see `RecvError::Lagged` and skip ahead.
    event_broadcast: tokio::sync::broadcast::Sender<HostEvent>,
    /// Single-writer lock for the state directory, acquired in `open`
    /// before any database is opened and held for the host's lifetime.
    /// See [`crate::instance_lock`].
    _instance_lock: crate::instance_lock::InstanceLock,
}

impl RuntimeHost {
    /// Open shared infrastructure and create an empty agent network.
    pub async fn open(config: &RuntimeConfig) -> Result<Self> {
        tracing::info!("RuntimeHost::open: delegating to SharedInfra::open");
        // Single-writer guard before any database is opened. A second
        // process holding the same state dir fails fast here instead of
        // double-writing the shared databases.
        let instance_lock = crate::instance_lock::acquire(&config.state_dir)?;
        let mut infra = SharedInfra::open(config).await?;
        tracing::info!("RuntimeHost::open: infrastructure ready, creating channels");
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
        let (registration_tx, registration_rx) = mpsc::unbounded_channel();
        let (notify_tx, notify_rx) = mpsc::unbounded_channel();
        // Broadcast channel for multi-consumer host events. Capacity 256:
        // even a busy agent at ~10 status changes/sec × 25 sec fits, and
        // subscribers that fall behind get `RecvError::Lagged` and skip.
        let (event_broadcast, _) = tokio::sync::broadcast::channel(256);
        let control = crate::control::HostControl::new(cmd_tx, event_broadcast.clone());
        infra.host_control = Some(control.clone());
        tracing::info!("RuntimeHost::open: host created successfully");
        Ok(Self {
            infra,
            network: AgentNetwork::new(),
            agents: HashMap::new(),
            event_tx,
            event_rx,
            cmd_rx,
            control,
            delegations: HashMap::new(),
            model: None,
            registration_rx,
            registration_tx,
            activation_pending: HashMap::new(),
            model_resolver: None,
            agent_idle_ttl: config.agent_idle_ttl,
            notify_tx,
            notify_rx,
            event_broadcast,
            _instance_lock: instance_lock,
        })
    }

    /// Returns a clonable [`HostControl`] for passing to agent tools.
    /// Agents use this to spawn peers, manage topology, send messages,
    /// and query system status.
    pub fn control(&self) -> crate::control::HostControl {
        self.control.clone()
    }

    /// Set the current model (for spawning agents from tools).
    pub fn set_model(&mut self, model: Arc<ArcSwapOption<Model>>) {
        self.model = Some(model);
    }

    /// Install the resolver used to materialize an agent's
    /// `preferred_model` during virtual-actor activation. The daemon
    /// wires its model-actor handle here (token rotation included).
    /// Without a resolver, only preference-less identities activate.
    pub fn set_model_resolver(&mut self, resolver: crate::model_actor::ModelHandle) {
        self.model_resolver = Some(resolver);
    }

    /// Receive the next event from any registered agent.
    ///
    /// This also handles delegation plumbing:
    /// - `TurnStarted` binds a delegation ID to a target turn/session.
    /// - `TurnCompleted` captures the accumulated response before the
    ///   compatibility `Done` event drains it, then resolves exactly the
    ///   delegation associated with that turn.
    ///
    /// The raw event is still returned to the caller for UI rendering.
    pub async fn recv_any(&mut self) -> Option<TaggedEvent> {
        let tagged = self.event_rx.recv().await?;
        Some(self.process_tagged(tagged))
    }

    /// Apply the in-band bookkeeping every received event must go through
    /// (delegation ledger, topology routing, status derivation) and return
    /// the event unchanged. Extracted from [`Self::recv_any`] so the
    /// multiplexed [`Self::recv_next`] shares one processing path.
    fn process_tagged(&mut self, tagged: TaggedEvent) -> TaggedEvent {
        let (name, event) = tagged;

        if matches!(
            &event,
            AgentEvent::TurnStarted { .. }
                | AgentEvent::TurnCompleted { .. }
                | AgentEvent::Done
                | AgentEvent::Error(_)
        ) && let Some(entry) = self.agents.get_mut(&name)
        {
            entry.inbound_pending.store(false, Ordering::Release);
        }

        match &event {
            AgentEvent::TurnStarted {
                turn_id,
                session_id,
                delegation_id,
                ..
            } => {
                if let Some(delegation_id) = delegation_id {
                    if let Some(record) = self.delegations.get_mut(delegation_id) {
                        record.snapshot.status = DelegationStatus::Running;
                        record.snapshot.turn_id = Some(*turn_id);
                        record.snapshot.session_id = Some(*session_id);
                        record.snapshot.updated_at = unix_epoch_ms();
                        let progress = record.progress.clone();
                        self.persist_delegation(*delegation_id);
                        self.persist_turn_start(&name, *turn_id, *session_id, Some(*delegation_id));
                        push_delegation_progress(
                            &progress,
                            "turn_started",
                            "running",
                            &format!("turn={turn_id} session={session_id}"),
                        );
                    }
                }
                self.persist_turn_start(&name, *turn_id, *session_id, *delegation_id);
            }
            AgentEvent::TurnCompleted {
                turn_id,
                session_id,
                delegation_id,
                status,
                telemetry,
                ..
            } => {
                let response = self.network.accumulated_response(&name).to_string();
                if let Some(delegation_id) = delegation_id {
                    if let Some(record) = self.delegations.get_mut(delegation_id) {
                        record.snapshot.status = match status {
                            agentik_types::TurnExecutionStatus::Completed => {
                                DelegationStatus::Completed
                            }
                            agentik_types::TurnExecutionStatus::Interrupted => {
                                DelegationStatus::Interrupted
                            }
                            agentik_types::TurnExecutionStatus::Failed => DelegationStatus::Failed,
                        };
                        record.snapshot.turn_id = Some(*turn_id);
                        record.snapshot.response = Some(response.clone());
                        record.snapshot.updated_at = unix_epoch_ms();
                        let progress = record.progress.clone();
                        let completion_status = record.snapshot.status;
                        let reply_tx = record.reply_tx.take();
                        self.persist_delegation(*delegation_id);
                        push_delegation_progress(
                            &progress,
                            "turn_completed",
                            completion_status.as_str(),
                            &status::truncate_preview(&response, 240),
                        );
                        if let Some(reply_tx) = reply_tx {
                            let _ = reply_tx.send(response);
                        }
                    }
                }
                self.persist_turn_finish(
                    &name,
                    *telemetry,
                    *turn_id,
                    *session_id,
                    *delegation_id,
                    *status,
                );
            }
            _ => {}
        }

        // Feed the event through the network (accumulates LlmResponse for
        // the delegation reply path and maintains termination/completion
        // bookkeeping). RoutingActions are intentionally ignored:
        // topology-edge message forwarding is retired — delegate_to is the
        // only inter-agent channel.
        let _ = self.network.process_event(&name, &event);

        // Phase 1: derive the agent's runtime status from the observed
        // event and notify subscribers. Done after network.process_event so
        // that `Done` reliably reflects the final-terminal state of the
        // turn (no later event will revert it back to Running unless a
        // fresh message arrives — which itself flips status again).
        self.observe_status(&name, &event);

        let (status, last_event) = derive_agent_status(&event);
        // Pure-noise events (text deltas, usage updates) change neither the
        // status nor the one-liner — skip the progress push entirely.
        if status.is_some() || last_event.is_some() {
            let status = status
                .as_ref()
                .map(|status| status.tag())
                .unwrap_or("active");
            for record in self.delegations.values_mut() {
                if record.snapshot.target_path == name
                    && record.snapshot.status == DelegationStatus::Running
                    && record.snapshot.turn_id.is_some()
                {
                    push_delegation_progress(
                        &record.progress,
                        "agent_event",
                        status,
                        last_event.as_deref().unwrap_or_default(),
                    );
                }
            }
        }

        (name, event)
    }

    /// Multiplexed receive for gateway-style drivers: awaits the next item
    /// from ANY of the host's input sources — agent events (with the same
    /// in-band processing as [`Self::recv_any`]), host lifecycle events,
    /// host commands / background spawn notices, or the virtual-actor
    /// idle-passivation sweep timer — in a single future.
    ///
    /// Single consumer, same contract as [`Self::recv_any`]: the daemon
    /// driver must be the only caller, or delegation bookkeeping and
    /// topology routing silently stop. Unlike calling the individual
    /// `recv_*` methods from a `tokio::select!` (which the TUI used to do
    /// via pointer aliasing), this method borrows the host once and keeps
    /// every channel fairly polled.
    pub async fn recv_next(&mut self) -> NextEvent {
        // Computed before the select — the deadline call borrows all of
        // `self`, which would fight the per-field channel borrows below.
        let passivation_at =
            tokio::time::Instant::from_std(self.next_passivation_deadline());
        tokio::select! {
            tagged = self.event_rx.recv() => {
                match tagged {
                    Some(tagged) => NextEvent::Agent(self.process_tagged(tagged)),
                    // The host itself holds `event_tx`, so the channel only
                    // closes if the host was torn down — park forever.
                    None => std::future::pending().await,
                }
            }
            notify = self.notify_rx.recv() => {
                match notify {
                    Some(event) => NextEvent::Host(event),
                    None => std::future::pending().await,
                }
            }
            cmd = self.cmd_rx.recv() => {
                if let Some(cmd) = cmd {
                    self.process_command(cmd);
                }
                NextEvent::Command
            }
            reg = self.registration_rx.recv() => {
                match reg {
                    Some(notice) => self.process_spawn_notice(notice),
                    None => std::future::pending().await,
                }
                NextEvent::Command
            }
            // Virtual-actor passivation sweep: a periodic tick that
            // evicts instances whose idle timer exceeded the TTL.
            _ = tokio::time::sleep_until(passivation_at) => {
                self.sweep_idle_agents();
                NextEvent::Command
            }
        }
    }

    /// Send a `HostEvent` to both the mpsc channel (TUI / `recv_event`)
    /// and the broadcast channel (event subscribers). The mpsc
    /// send is silently dropped if no receiver is alive; the broadcast
    /// send only fails if no receiver has ever subscribed (and we don't
    /// care in that case either — the broadcast keeps a 0-receiver
    /// buffer without panicking).
    fn emit_host_event(&self, event: HostEvent) {
        let _ = self.notify_tx.send(event.clone());
        let _ = self.event_broadcast.send(event);
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

    /// Spawn a new agent from an [`AgentProfileConfig`].
    ///
    /// Delegates to [`SharedInfra::spawn_agent`] — which only needs the
    /// shared infrastructure, not the topology network or registry.
    /// This allows callers to clone [`SharedInfra`] (cheap, all `Arc`)
    /// into async tasks without cloning the non-`Clone` [`RuntimeHost`].
    pub async fn spawn_agent(
        &self,
        agent_path: &agentik_types::AgentPath,
        config: &agentik_core::profile::AgentProfileConfig,
        global_model: Arc<ArcSwapOption<Model>>,
        model_override: Option<Model>,
    ) -> Result<AgentHandle> {
        self.infra
            .spawn_agent(agent_path, config, global_model, model_override)
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

    /// Returns a reference to the shared storage.
    pub fn storage(&self) -> &Arc<dyn AgentStorage> {
        &self.infra.storage
    }

    /// Returns a clone of the shared infra (for advanced use).
    pub fn infra(&self) -> SharedInfra {
        self.infra.clone()
    }

    /// Returns a reference to the persistent topology network. The network
    /// is kept for response accumulation and termination bookkeeping only
    /// (the delegation reply path reads `accumulated_response`); its
    /// routing actions are intentionally ignored — `delegate_to` is the
    /// only inter-agent channel.
    pub fn network(&self) -> &AgentNetwork {
        &self.network
    }

    /// Get a client for the named agent's isolated DataEngine session.
    ///
    /// Interactive UI views share the same session identity as the agent, so
    /// this is a read-only projection of exactly the DAG that agent's tools
    /// mutate—not a separate global graph.
    pub fn data_engine_client(&self, session_id: &str) -> DataEngineClient {
        self.infra.engine_manager.client_for_agent(session_id)
    }
}

fn unix_epoch_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|value| value.as_millis() as i64)
        .unwrap_or(0)
}
