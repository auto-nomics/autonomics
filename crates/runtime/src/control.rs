//! Host control channel — allows agent tools to command the RuntimeHost.
//!
//! Agents receive a [`HostControl`] (a clonable channel sender) via their
//! tool set. When a tool is invoked, it sends a [`HostCommand`] through the
//! channel. The RuntimeHost drains and executes these commands in its event
//! loop via [`RuntimeHost::try_process_commands`](crate::RuntimeHost::try_process_commands).
//!
//! Commands that need a response (Spawn, Delegate, GetStatus) include a
//! `oneshot` reply channel; the tool `await`s it.

use agentik_network::{EdgeTrigger, TerminationSpec};
use agentik_sdk::model::Model;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc::UnboundedSender;
use tokio::sync::oneshot;

/// A clonable handle for sending commands to RuntimeHost.
#[derive(Clone)]
pub struct HostControl {
    pub(crate) cmd_tx: UnboundedSender<HostCommand>,
    /// Broadcast sender for `HostEvent` stream. Cloned cheaply (Arc
    /// internally); each subscriber gets its own lag-tracked receiver.
    /// `None` for tests / pre-initialization contexts.
    pub(crate) event_broadcast:
        Option<tokio::sync::broadcast::Sender<crate::host::HostEvent>>,
}

impl HostControl {
    pub fn new(
        cmd_tx: UnboundedSender<HostCommand>,
        event_broadcast: tokio::sync::broadcast::Sender<crate::host::HostEvent>,
    ) -> Self {
        Self {
            cmd_tx,
            event_broadcast: Some(event_broadcast),
        }
    }

    /// Construct a `HostControl` without a broadcast subscription (for
    /// tests or contexts where event streaming isn't needed). The
    /// `subscribe_events` and `wait_agent` methods will return `None`
    /// / fail-soft when constructed this way.
    #[cfg(test)]
    pub fn new_without_broadcast(cmd_tx: UnboundedSender<HostCommand>) -> Self {
        Self {
            cmd_tx,
            event_broadcast: None,
        }
    }

    /// Subscribe to the host's event stream (agent registrations, status
    /// changes, shutdowns). Returns `None` if the host doesn't have a
    /// broadcast channel (e.g. unit-test `HostControl` constructed via
    /// [`new_without_broadcast`](Self::new_without_broadcast)).
    ///
    /// Each subscriber gets its own independent lag counter — multiple
    /// `wait_agent` calls can subscribe concurrently without interfering
    /// with each other or with the TUI's mpsc event channel.
    pub fn subscribe_events(
        &self,
    ) -> Option<tokio::sync::broadcast::Receiver<crate::host::HostEvent>> {
        self.event_broadcast.as_ref().map(|tx| tx.subscribe())
    }

    /// Send a fire-and-forget command (no response needed).
    fn fire(&self, cmd: HostCommand) {
        let _ = self.cmd_tx.send(cmd);
    }

    /// Send a command and await a typed response.
    async fn ask<T, F>(&self, make_cmd: F) -> Option<T>
    where
        F: FnOnce(oneshot::Sender<T>) -> HostCommand,
    {
        let (tx, rx) = oneshot::channel();
        if self.cmd_tx.send(make_cmd(tx)).is_err() {
            return None;
        }
        rx.await.ok()
    }

    // ── Convenience methods ────────────────────────────────

    /// Deliver a user-typed message to a named agent (used by the TUI).
    /// This is NOT inter-agent fire-and-forget — it is the user → agent
    /// communication channel.
    pub fn deliver_message(&self, name: &str, message: impl Into<String>) {
        self.fire(HostCommand::DeliverMessage {
            name: name.into(),
            message: message.into(),
        });
    }

    /// Delegate a task to a named agent and wait for its Done response.
    /// Returns the target agent's full response text.
    pub async fn delegate(&self, to: &str, message: impl Into<String>) -> Option<String> {
        self.ask(|tx| HostCommand::Delegate {
            to: to.into(),
            message: message.into(),
            reply_tx: tx,
        })
        .await
    }

    /// Fire-and-forget inter-agent message (Phase 5). Delivers `message`
    /// to agent `to` without waiting for a response. Returns:
    /// - `Some(Ok(()))` — agent found, message enqueued
    /// - `Some(Err(msg))` — agent not found or host resolved the name but
    ///   the agent was unregistered concurrently
    /// - `None` — host command channel closed (host shutting down)
    ///
    /// Unlike [`Self::delegate`], the caller continues immediately. If the
    /// target agent is mid-turn, the message is queued and processed on
    /// the next turn (same semantics as TUI's `deliver_message`).
    pub async fn send_message(
        &self,
        to: &str,
        message: impl Into<String>,
    ) -> Option<Result<(), String>> {
        self.ask(|tx| HostCommand::SendMessage {
            to: to.into(),
            message: message.into(),
            reply_tx: tx,
        })
        .await
    }

    pub fn shutdown_agent(&self, name: &str) {
        self.fire(HostCommand::Shutdown { name: name.into() });
    }

    pub fn add_node(&self, name: &str, profile: &str) {
        self.fire(HostCommand::AddNode {
            name: name.into(),
            profile: profile.into(),
            initial_prompt: None,
        });
    }

    pub fn add_node_with_prompt(&self, name: &str, profile: &str, prompt: impl Into<String>) {
        self.fire(HostCommand::AddNode {
            name: name.into(),
            profile: profile.into(),
            initial_prompt: Some(prompt.into()),
        });
    }

    pub fn remove_node(&self, name: &str) {
        self.fire(HostCommand::RemoveNode { name: name.into() });
    }

    pub fn connect(&self, from: &str, to: &str, trigger: EdgeTrigger) {
        self.fire(HostCommand::Connect {
            from: from.into(),
            to: to.into(),
            trigger,
        });
    }

    pub fn disconnect(&self, from: &str, to: &str) {
        self.fire(HostCommand::Disconnect {
            from: from.into(),
            to: to.into(),
        });
    }

    pub fn set_termination(&self, spec: TerminationSpec) {
        self.fire(HostCommand::SetTermination { spec });
    }

    pub fn reset_run_state(&self) {
        self.fire(HostCommand::ResetRunState);
    }

    pub fn inject_prompts(&self) {
        self.fire(HostCommand::InjectPrompts);
    }

    // ── Session management ──

    pub fn cancel_agent(&self, name: &str) {
        self.fire(HostCommand::CancelAgent { name: name.into() });
    }

    pub fn list_sessions(&self, name: &str) {
        self.fire(HostCommand::ListSessions { name: name.into() });
    }

    pub fn create_session(&self, name: &str, title: Option<String>, fork_from: Option<uuid::Uuid>) {
        self.fire(HostCommand::CreateSession {
            name: name.into(),
            title,
            fork_from,
        });
    }

    pub fn switch_session(&self, name: &str, session_id: uuid::Uuid) {
        self.fire(HostCommand::SwitchSession {
            name: name.into(),
            session_id,
        });
    }

    pub fn close_session(&self, name: &str, session_id: uuid::Uuid) {
        self.fire(HostCommand::CloseSession {
            name: name.into(),
            session_id,
        });
    }

    pub fn rename_session(&self, name: &str, session_id: uuid::Uuid, title: String) {
        self.fire(HostCommand::RenameSession {
            name: name.into(),
            session_id,
            title,
        });
    }

    // ── Model management ──

    pub fn set_agent_model(&self, name: &str, model: Model) {
        self.fire(HostCommand::SetAgentModel {
            name: name.into(),
            model,
        });
    }

    pub async fn agent_model_info(&self, name: &str) -> Option<(String, u64)> {
        self.ask(|tx| HostCommand::GetAgentModel {
            name: name.into(),
            reply_tx: tx,
        })
        .await
        .flatten()
    }

    pub async fn spawn_agent(
        &self,
        name: &str,
        caller_path: &agentik_types::AgentPath,
        caller_profile_path: &str,
        profile_segment: Option<&str>,
    ) -> Result<String, String> {
        self.ask(|tx| HostCommand::Spawn {
            name: name.into(),
            caller_path: caller_path.clone(),
            caller_profile_path: caller_profile_path.into(),
            profile_segment: profile_segment.map(String::from),
            reply_tx: tx,
        })
        .await
        .unwrap_or(Err("host command channel closed".into()))
    }

    /// Spawn and register an agent from a full profile + optional model
    /// override. Used by the TUI.
    pub async fn spawn_with_profile(
        &self,
        name: &str,
        caller_path: &agentik_types::AgentPath,
        profile: agentik_core::AgentProfile,
        model_override: Option<Model>,
    ) -> Result<String, String> {
        self.ask(|tx| HostCommand::SpawnWithProfile {
            name: name.into(),
            caller_path: caller_path.clone(),
            profile: Box::new(profile),
            model_override,
            reply_tx: tx,
        })
        .await
        .unwrap_or(Err("host command channel closed".into()))
    }

    /// Derive a specialized child profile from the caller's profile.
    pub async fn derive_profile(
        &self,
        caller_profile_path: &str,
        segment: &str,
        overrides: agentik_core::ProfileOverrides,
    ) -> Result<String, String> {
        self.ask(|tx| HostCommand::DeriveProfile {
            caller_profile_path: caller_profile_path.into(),
            segment: segment.into(),
            overrides: Box::new(overrides),
            reply_tx: tx,
        })
        .await
        .unwrap_or(Err("host command channel closed".into()))
    }

    pub async fn get_status(&self) -> Option<HostStatus> {
        self.ask(|tx| HostCommand::GetStatus { reply_tx: tx }).await
    }

    /// Route a task to the best-matching agent.
    /// `exclude` is the caller's own name to prevent self-routing.
    pub async fn route_task(
        &self,
        description: &str,
        exclude: Option<&str>,
    ) -> Option<RouteResult> {
        self.ask(|tx| HostCommand::RouteTask {
            description: description.into(),
            exclude: exclude.map(String::from),
            reply_tx: tx,
        })
        .await
    }

    /// Query detailed info about a specific agent.
    pub async fn get_agent_info(&self, name: &str) -> Option<AgentInfo> {
        self.ask(|tx| HostCommand::GetAgentInfo {
            name: name.into(),
            reply_tx: tx,
        })
        .await
        .flatten()
    }

    /// Block until `agent_name` reaches `Completed` / `Failed` or
    /// `timeout_ms` elapses. Returns `None` if the host command channel
    /// closed (host shutdown). On timeout, the returned `WaitAgentResult`
    /// carries `timed_out = true` and the snapshot status at that moment.
    ///
    /// If the agent is already in a terminal status at call time, returns
    /// immediately with that status — no polling, no wait task spawned.
    /// This matches codex v1 `wait` semantics.
    pub async fn wait_agent(
        &self,
        agent_name: &str,
        timeout_ms: u64,
    ) -> Option<Result<WaitAgentResult, String>> {
        self.ask(|tx| HostCommand::WaitAgentStatus {
            agent_name: agent_name.into(),
            timeout_ms,
            reply_tx: tx,
        })
        .await
    }

    /// List all agents persisted in the storage graph (Phase 4 query
    /// entry). Includes agents that are not currently registered with
    /// the host (e.g. leftover from a previous process run). The caller
    /// is responsible for filtering / joining with the in-memory
    /// registry.
    ///
    /// Returns `None` if the host command channel closed.
    pub async fn list_persisted_agents(
        &self,
    ) -> Option<Vec<agentik_core::storage::PersistedAgentGraph>> {
        self.ask(|tx| HostCommand::ListPersistedAgents { reply_tx: tx })
            .await
    }
}

/// Commands sent from agent tools to RuntimeHost via [`HostControl`].
#[allow(clippy::large_enum_variant)]
pub enum HostCommand {
    /// Spawn and register an agent. Reply: Ok(path_string) or Err(msg).
    /// `caller_path` is the parent agent's path; the child's path is
    /// derived as `caller_path.join(name)`.
    /// `caller_profile_path` is the caller's profile path for child profile
    /// lookup. `profile_segment` is None (reuse caller's profile), a relative
    /// segment (e.g. "genomics"), or an absolute profile path.
    Spawn {
        name: String,
        caller_path: agentik_types::AgentPath,
        caller_profile_path: String,
        profile_segment: Option<String>,
        reply_tx: oneshot::Sender<Result<String, String>>,
    },

    /// Spawn and register an agent from a full profile + optional model
    /// override. Used by the TUI for spawn-from-profile / restore flows.
    /// `caller_path` is the parent agent's path.
    SpawnWithProfile {
        name: String,
        caller_path: agentik_types::AgentPath,
        profile: Box<agentik_core::AgentProfile>,
        model_override: Option<Model>,
        reply_tx: oneshot::Sender<Result<String, String>>,
    },

    /// Shut down a named agent and remove from registry.
    Shutdown { name: String },

    /// Derive a child profile from the caller's profile. Reply: Ok(profile_path) or Err(msg).
    DeriveProfile {
        caller_profile_path: String,
        segment: String,
        overrides: Box<agentik_core::ProfileOverrides>,
        reply_tx: oneshot::Sender<Result<String, String>>,
    },

    /// Add a topology node.
    AddNode {
        name: String,
        profile: String,
        initial_prompt: Option<String>,
    },

    /// Remove a topology node.
    RemoveNode { name: String },

    /// Connect two nodes with a trigger.
    Connect {
        from: String,
        to: String,
        trigger: EdgeTrigger,
    },

    /// Disconnect two nodes.
    Disconnect { from: String, to: String },

    /// Deliver a user message to a named agent (TUI → agent).
    /// Not inter-agent communication — use Delegate for that.
    DeliverMessage { name: String, message: String },

    /// Fire-and-forget inter-agent message (Phase 5). Unlike Delegate,
    /// the sender does NOT wait for the target's Done response — the
    /// message is enqueued and the caller continues immediately.
    /// Reply: Ok(()) on successful delivery, Err(msg) if agent not found.
    SendMessage {
        to: String,
        message: String,
        reply_tx: oneshot::Sender<Result<(), String>>,
    },

    /// Delegate a task to an agent and wait for its Done response.
    /// Reply: the target agent's response text.
    Delegate {
        to: String,
        message: String,
        reply_tx: oneshot::Sender<String>,
    },

    /// Query host + topology status.
    GetStatus {
        reply_tx: oneshot::Sender<HostStatus>,
    },

    /// Set termination condition.
    SetTermination { spec: TerminationSpec },

    /// Reset routing state (keep topology).
    ResetRunState,

    /// Inject initial prompts for all nodes that have them.
    InjectPrompts,

    /// Route a task description to the best-matching agent.
    /// Reply: routing recommendation with candidates.
    /// `exclude` is the caller's own name (to prevent self-routing).
    RouteTask {
        description: String,
        exclude: Option<String>,
        reply_tx: oneshot::Sender<RouteResult>,
    },

    /// Query detailed info about a specific agent.
    GetAgentInfo {
        name: String,
        reply_tx: oneshot::Sender<Option<AgentInfo>>,
    },

    /// Block until a named agent reaches a terminal status (Completed
    /// or Failed) or the timeout elapses. Reply: `Ok(WaitAgentResult)`
    /// if the agent reached a final status, `Err(msg)` if the timeout
    /// elapsed, the agent wasn't found, or the broadcast channel closed.
    ///
    /// Modeled after codex's `multi_agents_v1::wait` tool
    /// (`codex-rs/core/src/tools/handlers/multi_agents/wait.rs:46-222`):
    /// subscribe to the per-agent status stream, then poll until the
    /// status is final. Autonomics uses the host's broadcast event
    /// channel (`HostEvent::AgentStatusChanged`) instead of a per-agent
    /// `watch::Sender<AgentStatus>` because the runtime already derives
    /// status centrally in `RuntimeHost::observe_status`.
    WaitAgentStatus {
        agent_name: String,
        timeout_ms: u64,
        reply_tx: oneshot::Sender<Result<WaitAgentResult, String>>,
    },

    /// List all agents currently persisted in the storage graph
    /// (`agent_graph` table). Includes both live agents (registered with
    /// this host) AND agents persisted by previous process runs that may
    /// have been shut down or orphaned. Reply: persisted rows ordered by
    /// `updated_at DESC` (most recent first).
    ///
    /// Phase 4 query entry. Mirrors codex's `AgentGraphStore::list()`.
    ListPersistedAgents {
        reply_tx: oneshot::Sender<Vec<agentik_core::storage::PersistedAgentGraph>>,
    },

    // ── Session management ──
    /// Cancel the current turn of a named agent.
    CancelAgent { name: String },

    /// Request session list from a named agent.
    ListSessions { name: String },

    /// Create a new session in a named agent.
    CreateSession {
        name: String,
        title: Option<String>,
        fork_from: Option<uuid::Uuid>,
    },

    /// Switch the active session of a named agent.
    SwitchSession {
        name: String,
        session_id: uuid::Uuid,
    },

    /// Close a session in a named agent.
    CloseSession {
        name: String,
        session_id: uuid::Uuid,
    },

    /// Rename a session in a named agent.
    RenameSession {
        name: String,
        session_id: uuid::Uuid,
        title: String,
    },

    // ── Model management ──
    /// Hot-swap the model of a named agent.
    SetAgentModel { name: String, model: Model },

    /// Query model info (name + context length) for a named agent.
    GetAgentModel {
        name: String,
        reply_tx: oneshot::Sender<Option<(String, u64)>>,
    },
}

/// Read-only snapshot of host + topology state, returned by GetStatus.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HostStatus {
    /// All registered (running) agents with their capabilities.
    pub agents: Vec<AgentInfo>,
    /// Available profiles (blueprints that can be spawned).
    pub profiles: Vec<AgentInfo>,
    /// All topology node names.
    pub nodes: Vec<String>,
    /// Total edge count.
    pub edge_count: usize,
    /// Whether the topology contains a cycle.
    pub is_cyclic: bool,
    /// Root nodes (no incoming edges).
    pub roots: Vec<String>,
    /// Leaf nodes (no outgoing edges).
    pub leaves: Vec<String>,
    /// Current round count (max completions across nodes).
    pub rounds: usize,
    /// Whether the network has terminated.
    pub is_finished: bool,
    /// Current termination spec (as debug string).
    pub termination: String,
}

/// Runtime lifecycle state of a registered agent.
///
/// Derived from the [`agentik_sdk::AgentEvent`] stream by
/// [`crate::RuntimeHost::recv_any`]. Surfaces the live activity of every
/// spawned agent so callers (`list_agents`, `get_agent_info`) can answer
/// "what is agent X doing right now?" without polling the agent itself.
///
/// Lifecycle:
///
/// ```text
/// Idle ──► Running ──► AwaitingTool { tool } ──► Running ──► …
///   │         │                                     │
///   │         └──► Completed                        └──► Failed(msg)
///   └──► Completed / Failed   (one-shot tasks that never turned)
/// ```
///
/// `Completed` is a terminal state: a fresh agent turn starts back at
/// `Running` (or `AwaitingTool`) on the next message. `Failed(msg)` is
/// sticky until the agent is shut down — the message captures the most
/// recent error.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AgentStatus {
    /// No message has been delivered yet, or the agent is between turns.
    Idle,
    /// The agent is mid-turn — either streaming an LLM response or about
    /// to call another tool. `last_event` may carry a one-line summary.
    Running,
    /// The agent issued a tool call and is waiting for its result.
    /// `tool` is the registered tool name (e.g. `run_bash`).
    AwaitingTool { tool: String },
    /// The agent emitted `Done` for the current turn. Stays until the
    /// next message flips it back to `Running`. Not sticky across shutdown.
    Completed,
    /// The agent emitted `Error`. Sticky until shutdown.
    Failed { message: String },
}

impl AgentStatus {
    /// Short lowercase tag suitable for log lines and JSON.
    pub fn tag(&self) -> &'static str {
        match self {
            AgentStatus::Idle => "idle",
            AgentStatus::Running => "running",
            AgentStatus::AwaitingTool { .. } => "awaiting_tool",
            AgentStatus::Completed => "completed",
            AgentStatus::Failed { .. } => "failed",
        }
    }
}

/// Information about one registered agent, including capability metadata
/// and live runtime status.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentInfo {
    /// Agent short name (last path segment, e.g. `researcher`).
    pub name: String,
    /// Full hierarchical path (e.g. `/root/researcher/worker`).
    pub path: String,
    /// Human/LLM-readable capability summary.
    pub summary: String,
    /// Capability tags for quick filtering.
    pub tags: Vec<String>,
    /// Areas of expertise.
    pub expertise: Vec<String>,
    /// Tool names available to this agent.
    pub tools: Vec<String>,
    /// Live runtime status, derived from the agent's event stream.
    /// Defaults to [`AgentStatus::Idle`] when no message has been
    /// delivered yet.
    #[serde(default = "default_agent_status")]
    pub status: AgentStatus,
    /// One-line summary of the most recent event (e.g. the tool name on
    /// `AwaitingTool`, the failing message on `Failed`). `None` until
    /// the first event arrives.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_event: Option<String>,
}

fn default_agent_status() -> AgentStatus {
    AgentStatus::Idle
}

/// A routing recommendation returned by [`HostControl::route_task`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RouteResult {
    /// Name of the recommended agent.
    pub agent: String,
    /// Why this agent was selected (human-readable).
    pub reason: String,
    /// Match score (higher = better).
    pub score: f64,
    /// All candidates that were considered, with their scores.
    pub candidates: Vec<RouteCandidate>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RouteCandidate {
    pub agent: String,
    pub score: f64,
    pub matched_tags: Vec<String>,
}

/// Result of a `wait_agent` call: the agent's final status and
/// whether the wait timed out before reaching it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WaitAgentResult {
    /// Resolved agent full path (e.g. `/root/researcher`).
    pub agent_path: String,
    /// The final status observed. If `timed_out`, this is the status
    /// at the time of timeout (typically `Running` or `AwaitingTool`).
    pub status: AgentStatus,
    /// Last event summary from when this status was observed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_event: Option<String>,
    /// `true` if the timeout fired before the agent reached a terminal
    /// status. In that case `status` is non-final and `last_event` is
    /// the snapshot from timeout time.
    pub timed_out: bool,
}
