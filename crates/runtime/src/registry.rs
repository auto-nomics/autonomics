//! Agent registry — owns agent handles with multiplexed event access.
//!
//! Each registered agent gets a background relay task that forwards
//! `AgentEvent`s to a central channel. This solves the fundamental problem
//! that `tokio::select!` can't poll a dynamic number of
//! `AgentHandle::recv_event()` futures — the registry provides a single
//! [`recv_any`](AgentRegistry::recv_any) call that receives from any agent.
//!
//! ## Separation of concerns
//!
//! - **RuntimeHost** — shared infrastructure + agent factory (`spawn_agent`).
//! - **AgentRegistry** — owns handles, provides multiplexed event access
//!   and targeted message delivery (`send_to`, `recv_any`, `shutdown`).
//! - **AgentNetwork** (in `agentik-network`) — pure topology routing logic,
//!   no I/O.
//!
//! The caller wires them together: spawn via host → register → feed events
//! to AgentNetwork → execute routing actions via registry.

use std::collections::HashMap;

use agentik_sdk::types::AgentEvent;
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use tokio::task::JoinHandle;

use crate::host::AgentHandle;

/// Commands sent from the registry to a per-agent relay task.
enum AgentCommand {
    /// Inject a user message into the agent.
    Message(String),
    /// Shut the agent down.
    Shutdown,
}

/// Internal entry for one registered agent.
struct AgentEntry {
    /// Send commands to this agent's relay task.
    cmd_tx: UnboundedSender<AgentCommand>,
    /// Join handle for the relay task — kept alive so the task isn't dropped.
    _relay_task: JoinHandle<()>,
}

/// An `AgentEvent` tagged with the name of the agent that produced it.
pub type TaggedEvent = (String, AgentEvent);

/// Owns a pool of agent handles with multiplexed event access.
///
/// Each registered agent has a background relay task that forwards
/// `AgentEvent`s to a central channel. Call
/// [`recv_any`](Self::recv_any) to receive the next event from any agent.
///
/// This struct is **not** `Clone` — it owns agent handles and the
/// multiplexed event receiver. The caller should keep a single instance
/// for the lifetime of the agent pool.
///
/// ## Example
///
/// ```ignore
/// let mut registry = AgentRegistry::new();
///
/// // Spawn an agent and register it.
/// let handle = host.spawn_agent("writer", &profile, model, None).await?;
/// registry.register(handle);
///
/// // Send a message to a named agent.
/// registry.send_to("writer", "hello".into());
///
/// // Receive events from any agent.
/// while let Some((name, event)) = registry.recv_any().await {
///     println!("[{name}] {event:?}");
/// }
/// ```
pub struct AgentRegistry {
    /// Per-agent relay entries (command channels + task handles).
    agents: HashMap<String, AgentEntry>,
    /// Multiplexed event receiver — drains events from all relay tasks.
    event_rx: UnboundedReceiver<TaggedEvent>,
    /// Clone of the sender — used when registering new agents (each relay
    /// task gets its own clone).
    event_tx: UnboundedSender<TaggedEvent>,
}

impl AgentRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        Self {
            agents: HashMap::new(),
            event_rx,
            event_tx,
        }
    }

    /// Register an agent. Takes ownership of the [`AgentHandle`] — a
    /// background relay task will own it and forward events.
    ///
    /// After registration, use [`send_to`](Self::send_to) to send messages
    /// and [`recv_any`](Self::recv_any) to receive events. Do not use the
    /// handle directly after registration.
    pub fn register(&mut self, handle: AgentHandle) {
        let name = handle.name.clone();
        let relay_name = name.clone();
        let event_tx = self.event_tx.clone();
        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel::<AgentCommand>();

        let relay_task = tokio::spawn(async move {
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

    /// Send a message to a named agent.
    pub fn send_to(&self, name: &str, message: String) {
        if let Some(entry) = self.agents.get(name) {
            let _ = entry.cmd_tx.send(AgentCommand::Message(message));
        }
    }

    /// Shut down a named agent and remove it from the registry.
    pub fn shutdown_agent(&mut self, name: &str) {
        if let Some(entry) = self.agents.remove(name) {
            let _ = entry.cmd_tx.send(AgentCommand::Shutdown);
        }
    }

    /// Shut down all registered agents.
    pub fn shutdown_all(&mut self) {
        for (_, entry) in self.agents.drain() {
            let _ = entry.cmd_tx.send(AgentCommand::Shutdown);
        }
    }

    /// Receive the next event from any registered agent.
    ///
    /// Returns `None` when all relay tasks have exited (all agents are
    /// shut down or their event streams closed).
    pub async fn recv_any(&mut self) -> Option<TaggedEvent> {
        self.event_rx.recv().await
    }

    /// Try to receive an event without blocking.
    pub fn try_recv_any(&mut self) -> Option<TaggedEvent> {
        self.event_rx.try_recv().ok()
    }

    /// Check if an agent with the given name is registered.
    pub fn contains(&self, name: &str) -> bool {
        self.agents.contains_key(name)
    }

    /// Number of registered agents.
    pub fn len(&self) -> usize {
        self.agents.len()
    }

    /// Is the registry empty?
    pub fn is_empty(&self) -> bool {
        self.agents.is_empty()
    }

    /// Names of all registered agents.
    pub fn agent_names(&self) -> Vec<&str> {
        self.agents.keys().map(|s| s.as_str()).collect()
    }
}

impl Default for AgentRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for AgentRegistry {
    fn drop(&mut self) {
        // Best-effort shutdown of all relay tasks.
        for (_, entry) in self.agents.drain() {
            let _ = entry.cmd_tx.send(AgentCommand::Shutdown);
        }
    }
}

/// The relay loop — owns one [`AgentHandle`] and bridges:
/// - **Inbound**: `AgentCommand`s from the registry → `send_message`/`shutdown`
/// - **Outbound**: `AgentEvent`s from the agent → tagged events to registry
///
/// This indirection exists because `AgentHandle` is a monolithic struct —
/// the registry can't hold `&mut` references to multiple handles
/// simultaneously in a `select!`. The relay task bridges that gap.
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

            // Forward agent events to the registry.
            event = handle.recv_event() => match event {
                Some(ev) => {
                    if event_tx.send((name.clone(), ev)).is_err() {
                        tracing::debug!(
                            agent = %name,
                            "registry channel closed, relay exiting"
                        );
                        break;
                    }
                }
                None => {
                    tracing::debug!(
                        agent = %name,
                        "agent event stream closed, relay exiting"
                    );
                    break;
                }
            },

            // Forward registry commands to the agent.
            cmd = cmd_rx.recv() => match cmd {
                Some(AgentCommand::Message(text)) => {
                    handle.send_message(text);
                }
                Some(AgentCommand::Shutdown) | None => {
                    tracing::debug!(
                        agent = %name,
                        "shutdown command received, relay exiting"
                    );
                    handle.shutdown();
                    break;
                }
            }
        }
    }

    tracing::debug!(agent = %name, "relay task exited");
}
