//! Host control channel — allows agent tools to command the RuntimeHost.
//!
//! Agents receive a [`HostControl`] (a clonable channel sender) via their
//! tool set. When a tool is invoked, it sends a [`HostCommand`] through the
//! channel. The RuntimeHost drains and executes these commands in its event
//! loop via [`RuntimeHost::try_process_commands`](crate::RuntimeHost::try_process_commands).
//!
//! Commands that need a response (Spawn, Delegate, GetStatus) include a
//! `oneshot` reply channel; the tool `await`s it.

use agentik_core::tools::ProgressBuffer;
use agentik_core::{AgentRuntimeConfig, AgentRuntimeOverrides};
use agentik_sdk::model::Model;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc::UnboundedSender;
use tokio::sync::oneshot;
use uuid::Uuid;

/// How long [`HostControl::deliver_message_tracked`] waits for the host
/// loop to confirm delivery before giving up (`None`). Generous on purpose:
/// the host loop also drains agent events, so a busy runtime can sit on a
/// command for a while — this only fires when the loop is wedged or gone.
const DELIVERY_CONFIRM_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// A clonable handle for sending commands to RuntimeHost.
#[derive(Clone)]
pub struct HostControl {
    pub(crate) cmd_tx: UnboundedSender<HostCommand>,
    /// Broadcast sender for `HostEvent` stream. Cloned cheaply (Arc
    /// internally); each subscriber gets its own lag-tracked receiver.
    /// `None` for tests / pre-initialization contexts.
    pub(crate) event_broadcast: Option<tokio::sync::broadcast::Sender<crate::host::HostEvent>>,
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
    /// `subscribe_events` method will return `None` when constructed this way.
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
    /// callers can subscribe concurrently without interfering with each
    /// other or with the TUI's mpsc event channel.
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
            reply_tx: None,
        });
    }

    /// Deliver a user message and await the host's delivery confirmation.
    ///
    /// Unlike [`deliver_message`](Self::deliver_message), an accepted call
    /// means the runtime actually resolved a live agent and enqueued the
    /// message. Returns `None` when the host loop is unreachable or stays
    /// silent for [`DELIVERY_CONFIRM_TIMEOUT`] — the historical failure mode
    /// where messages were acknowledged but silently dropped (empty registry
    /// after a restart, exited agent loop) surfaces as an error instead.
    pub async fn deliver_message_tracked(
        &self,
        name: &str,
        message: impl Into<String>,
    ) -> Option<Result<(), String>> {
        tokio::time::timeout(
            DELIVERY_CONFIRM_TIMEOUT,
            self.ask(|reply_tx| HostCommand::DeliverMessage {
                name: name.into(),
                message: message.into(),
                reply_tx: Some(reply_tx),
            }),
        )
        .await
        .ok()
        .flatten()
    }

    /// Delegate a task to a named agent and wait for its Done response.
    /// Returns the target agent's full response text.
    pub async fn delegate(
        &self,
        caller_path: &str,
        to: &str,
        message: impl Into<String>,
    ) -> Option<String> {
        self.delegate_tracked(
            to,
            message,
            Some(caller_path.to_string()),
            Uuid::new_v4(),
            None,
        )
        .await
    }

    /// Delegate with stable identity and an optional live progress sink.
    #[allow(clippy::too_many_arguments)]
    pub async fn delegate_tracked(
        &self,
        to: &str,
        message: impl Into<String>,
        caller_path: Option<String>,
        delegation_id: Uuid,
        progress: Option<ProgressBuffer>,
    ) -> Option<String> {
        self.ask(|tx| HostCommand::Delegate {
            to: to.into(),
            message: message.into(),
            caller_path,
            delegation_id,
            progress,
            reply_tx: tx,
        })
        .await
    }

    /// Shut down a named agent (operator path — unrestricted). Use
    /// [`Self::shutdown_child_agent`] for agent-initiated shutdowns, which
    /// the host restricts to the caller's direct children.
    pub fn shutdown_agent(&self, name: &str) {
        self.fire(HostCommand::Shutdown {
            name: name.into(),
            caller_path: None,
            reply_tx: None,
        });
    }

    /// Shut down one of the caller's direct child agents. Returns:
    /// - `Some(Ok(()))` — the child was shut down and unregistered
    /// - `Some(Err(msg))` — target is not a direct child of the caller
    /// - `None` — host loop unreachable or silent for
    ///   [`DELIVERY_CONFIRM_TIMEOUT`]
    pub async fn shutdown_child_agent(
        &self,
        caller_path: &str,
        name: &str,
    ) -> Option<Result<(), String>> {
        tokio::time::timeout(
            DELIVERY_CONFIRM_TIMEOUT,
            self.ask(|tx| HostCommand::Shutdown {
                name: name.into(),
                caller_path: Some(caller_path.into()),
                reply_tx: Some(tx),
            }),
        )
        .await
        .ok()
        .flatten()
    }

    /// Trigger manual compaction on a named agent's active session.
    pub fn compact_agent(&self, name: &str) {
        self.fire(HostCommand::CompactAgent { name: name.into() });
    }

    // ── Session management ──

    /// Cancel the current turn of a named agent (operator path —
    /// unrestricted). Use [`Self::interrupt_child_agent`] for
    /// agent-initiated interrupts, which the host restricts to the
    /// caller's direct children.
    pub fn cancel_agent(&self, name: &str) {
        self.fire(HostCommand::CancelAgent {
            name: name.into(),
            caller_path: None,
            reply_tx: None,
        });
    }

    /// Interrupt the current turn of one of the caller's direct child
    /// agents. Returns:
    /// - `Some(Ok(()))` — the cancel command was forwarded to the child
    /// - `Some(Err(msg))` — target is not a direct child of the caller
    /// - `None` — host loop unreachable or silent for
    ///   [`DELIVERY_CONFIRM_TIMEOUT`]
    pub async fn interrupt_child_agent(
        &self,
        caller_path: &str,
        name: &str,
    ) -> Option<Result<(), String>> {
        tokio::time::timeout(
            DELIVERY_CONFIRM_TIMEOUT,
            self.ask(|tx| HostCommand::CancelAgent {
                name: name.into(),
                caller_path: Some(caller_path.into()),
                reply_tx: Some(tx),
            }),
        )
        .await
        .ok()
        .flatten()
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

    /// Layer runtime overrides onto a live agent and return its resolved config.
    pub async fn set_agent_runtime_config(
        &self,
        name: &str,
        overrides: AgentRuntimeOverrides,
    ) -> Result<AgentRuntimeConfig, String> {
        self.ask(|tx| HostCommand::SetAgentRuntimeConfig {
            name: name.into(),
            overrides: Box::new(overrides),
            reply_tx: tx,
        })
        .await
        .unwrap_or(Err("host command channel closed".into()))
    }

    /// Spawn and register an agent from the caller's kind (or an explicit
    /// role segment). Used by the LLM-facing `spawn_agent` tool.
    pub async fn spawn_agent(
        &self,
        name: &str,
        caller_path: &agentik_types::AgentPath,
        caller_kind: agentik_core::AgentKind,
        profile_segment: Option<&str>,
    ) -> Result<String, String> {
        let kind = match profile_segment {
            None => caller_kind,
            Some(seg) => match agentik_core::AgentKind::from_name(seg) {
                Some(kind) => kind,
                None => {
                    return Err(format!(
                        "Unknown role '{seg}'. Use researcher or developer."
                    ));
                }
            },
        };
        self.ask(|tx| HostCommand::Spawn {
            name: name.into(),
            caller_path: caller_path.clone(),
            config: Box::new(agentik_core::AgentProfileConfig::new(kind)),
            model_override: None,
            reply_tx: tx,
        })
        .await
        .unwrap_or(Err("host command channel closed".into()))
    }

    /// Spawn and register an agent from a full per-agent config + optional
    /// model override. Used by the TUI, gateway, and restore flows.
    pub async fn spawn_with_config(
        &self,
        name: &str,
        caller_path: &agentik_types::AgentPath,
        config: agentik_core::AgentProfileConfig,
        model_override: Option<Model>,
    ) -> Result<String, String> {
        self.ask(|tx| HostCommand::Spawn {
            name: name.into(),
            caller_path: caller_path.clone(),
            config: Box::new(config),
            model_override,
            reply_tx: tx,
        })
        .await
        .unwrap_or(Err("host command channel closed".into()))
    }

    pub async fn get_status(&self) -> Option<HostStatus> {
        self.ask(|tx| HostCommand::GetStatus { reply_tx: tx }).await
    }

    /// Route a task to the best-matching agent. Live-agent candidates are
    /// restricted to `caller_path`'s direct children; the spawnable agent
    /// kinds are always considered.
    pub async fn route_task(&self, description: &str, caller_path: &str) -> Option<RouteResult> {
        self.ask(|tx| HostCommand::RouteTask {
            description: description.into(),
            caller_path: caller_path.into(),
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

    /// List delegation records, newest first.
    pub async fn list_delegations(
        &self,
        caller_path: Option<&str>,
        target_path: Option<&str>,
        status: Option<&str>,
    ) -> Option<Vec<DelegationSnapshot>> {
        self.ask(|tx| HostCommand::ListDelegations {
            caller_path: caller_path.map(str::to_string),
            target_path: target_path.map(str::to_string),
            status: status.map(str::to_string),
            reply_tx: tx,
        })
        .await
    }

    /// Read the persisted execution history for a live agent.
    pub async fn agent_history(
        &self,
        agent_name: &str,
        limit: usize,
    ) -> Option<AgentExecutionHistory> {
        self.ask(|tx| HostCommand::GetAgentHistory {
            agent_name: agent_name.into(),
            limit,
            reply_tx: tx,
        })
        .await
    }
}

/// Commands sent from agent tools to RuntimeHost via [`HostControl`].
#[allow(clippy::large_enum_variant)]
pub enum HostCommand {
    /// Spawn and register an agent. Reply: Ok(path_string) or Err(msg).
    /// `caller_path` is the parent agent's path; the child's path is
    /// derived as `caller_path.join(name)`. `config` carries the agent
    /// kind plus per-agent model/runtime preferences.
    Spawn {
        name: String,
        caller_path: agentik_types::AgentPath,
        config: Box<agentik_core::AgentProfileConfig>,
        model_override: Option<Model>,
        reply_tx: oneshot::Sender<Result<String, String>>,
    },

    /// Shut down a named agent and remove from registry.
    /// `caller_path` is `None` for operator callers (unrestricted) and
    /// `Some(agent path)` for agent callers, in which case the host only
    /// allows the agent's direct children. `reply_tx` is `Some` for
    /// agent-facing calls so denials are surfaced.
    Shutdown {
        name: String,
        caller_path: Option<String>,
        reply_tx: Option<oneshot::Sender<std::result::Result<(), String>>>,
    },

    /// Deliver a user message to a named agent (TUI → agent).
    /// Not inter-agent communication — use Delegate for that.
    ///
    /// `reply_tx` is `None` for the fire-and-forget TUI path. The gateway
    /// sets it so delivery failures (unknown agent, exited loop) are
    /// reported to the HTTP caller instead of being acknowledged with a
    /// blind 202. Reply: `Ok(())` once enqueued on the agent's command
    /// channel, `Err(msg)` explaining why the message was dropped.
    DeliverMessage {
        name: String,
        message: String,
        reply_tx: Option<oneshot::Sender<Result<(), String>>>,
    },

    /// Delegate a task to an agent and wait for its Done response.
    /// Reply: the target agent's response text.
    Delegate {
        to: String,
        message: String,
        caller_path: Option<String>,
        delegation_id: Uuid,
        progress: Option<ProgressBuffer>,
        reply_tx: oneshot::Sender<String>,
    },

    /// Query first-class delegation records.
    ListDelegations {
        caller_path: Option<String>,
        target_path: Option<String>,
        status: Option<String>,
        reply_tx: oneshot::Sender<Vec<DelegationSnapshot>>,
    },

    /// Read an agent's persisted conversation history.
    GetAgentHistory {
        agent_name: String,
        limit: usize,
        reply_tx: oneshot::Sender<AgentExecutionHistory>,
    },

    /// Query host + topology status.
    GetStatus {
        reply_tx: oneshot::Sender<HostStatus>,
    },

    /// Route a task description to the best-matching agent.
    /// Reply: routing recommendation with candidates.
    /// Live-agent candidates are restricted to the direct children of
    /// `caller_path`.
    RouteTask {
        description: String,
        caller_path: String,
        reply_tx: oneshot::Sender<RouteResult>,
    },

    /// Query detailed info about a specific agent.
    GetAgentInfo {
        name: String,
        reply_tx: oneshot::Sender<Option<AgentInfo>>,
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
    /// Cancel the current turn of a named agent. `caller_path` is `None`
    /// for operator callers (unrestricted) and `Some(agent path)` for
    /// agent callers, in which case the host only allows the agent's
    /// direct children. `reply_tx` is `Some` for agent-facing calls so
    /// denials are surfaced.
    CancelAgent {
        name: String,
        caller_path: Option<String>,
        reply_tx: Option<oneshot::Sender<std::result::Result<(), String>>>,
    },

    /// Trigger manual compaction on a named agent's active session.
    CompactAgent { name: String },

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

    /// Set per-agent runtime overrides. Reply: effective config or error.
    SetAgentRuntimeConfig {
        name: String,
        overrides: Box<AgentRuntimeOverrides>,
        reply_tx: oneshot::Sender<Result<AgentRuntimeConfig, String>>,
    },

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

/// Lifecycle status of a first-class delegation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DelegationStatus {
    Pending,
    Running,
    Completed,
    Interrupted,
    Failed,
}

impl DelegationStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Running => "running",
            Self::Completed => "completed",
            Self::Interrupted => "interrupted",
            Self::Failed => "failed",
        }
    }
}

/// Queryable record for one agent-to-agent task.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DelegationSnapshot {
    pub delegation_id: Uuid,
    pub caller_path: Option<String>,
    pub target_path: String,
    pub task: String,
    pub status: DelegationStatus,
    pub turn_id: Option<Uuid>,
    pub session_id: Option<Uuid>,
    pub response: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

/// Restored conversation history for a live agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentExecutionHistory {
    pub agent_path: String,
    pub agent_id: Uuid,
    pub session_id: Option<Uuid>,
    pub messages: Vec<agentik_sdk::types::messages::Message>,
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
    /// Runtime identity used to locate this agent's persisted history.
    #[serde(default)]
    pub agent_id: Option<Uuid>,
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
