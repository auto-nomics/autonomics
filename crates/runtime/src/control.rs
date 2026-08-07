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
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc::UnboundedSender;
use tokio::sync::oneshot;

/// A clonable handle for sending commands to RuntimeHost.
#[derive(Clone)]
pub struct HostControl {
    pub(crate) cmd_tx: UnboundedSender<HostCommand>,
}

impl HostControl {
    pub fn new(cmd_tx: UnboundedSender<HostCommand>) -> Self {
        Self { cmd_tx }
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

    /// Send a message to a named agent (fire-and-forget, no response).
    pub fn send_to(&self, name: &str, message: impl Into<String>) {
        self.fire(HostCommand::SendTo {
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

    pub fn add_node_with_prompt(
        &self,
        name: &str,
        profile: &str,
        prompt: impl Into<String>,
    ) {
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

    pub async fn spawn_agent(
        &self,
        name: &str,
        profile_name: &str,
    ) -> Result<String, String> {
        self.ask(|tx| HostCommand::Spawn {
            name: name.into(),
            profile_name: profile_name.into(),
            reply_tx: tx,
        })
        .await
        .unwrap_or(Err("host command channel closed".into()))
    }

    pub async fn get_status(&self) -> Option<HostStatus> {
        self.ask(|tx| HostCommand::GetStatus { reply_tx: tx }).await
    }

    /// Route a task to the best-matching agent.
    pub async fn route_task(&self, description: &str) -> Option<RouteResult> {
        self.ask(|tx| HostCommand::RouteTask {
            description: description.into(),
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
}

/// Commands sent from agent tools to RuntimeHost via [`HostControl`].
#[derive(Debug)]
pub enum HostCommand {
    /// Spawn and register an agent. Reply: Ok(name) or Err(msg).
    Spawn {
        name: String,
        profile_name: String,
        reply_tx: oneshot::Sender<Result<String, String>>,
    },

    /// Shut down a named agent and remove from registry.
    Shutdown { name: String },

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

    /// Send a message to a named agent (fire-and-forget).
    SendTo { name: String, message: String },

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
    RouteTask {
        description: String,
        reply_tx: oneshot::Sender<RouteResult>,
    },

    /// Query detailed info about a specific agent.
    GetAgentInfo {
        name: String,
        reply_tx: oneshot::Sender<Option<AgentInfo>>,
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

/// Information about one registered agent, including capability metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentInfo {
    /// Agent name (unique within the host).
    pub name: String,
    /// Human/LLM-readable capability summary.
    pub summary: String,
    /// Capability tags for quick filtering.
    pub tags: Vec<String>,
    /// Areas of expertise.
    pub expertise: Vec<String>,
    /// Tool names available to this agent.
    pub tools: Vec<String>,
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
