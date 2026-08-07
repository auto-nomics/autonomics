//! Host control channel — allows agent tools to command the RuntimeHost.
//!
//! Agents receive a [`HostControl`] (a clonable channel sender) via their
//! tool set. When a tool is invoked, it sends a [`HostCommand`] through the
//! channel. The RuntimeHost drains and executes these commands in its event
//! loop via [`RuntimeHost::try_process_commands`](crate::RuntimeHost::try_process_commands).
//!
//! Commands that need a response (Spawn, GetStatus) include a `oneshot`
//! reply channel; the tool `await`s it. The latency is bounded by the
//! event-loop tick rate (~30 ms in the TUI).

use agentik_network::{EdgeTrigger, TerminationSpec};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc::UnboundedSender;
use tokio::sync::oneshot;

/// A clonable handle for sending commands to RuntimeHost.
///
/// Passed to agent tools so they can control the multi-agent system
/// (spawn agents, manage topology, send messages, query status).
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

    pub fn send_to(&self, name: &str, message: impl Into<String>) {
        self.fire(HostCommand::SendTo {
            name: name.into(),
            message: message.into(),
        });
    }

    pub fn shutdown_agent(&self, name: &str) {
        self.fire(HostCommand::Shutdown { name: name.into() });
    }

    pub fn add_node(&self, name: &str, profile: &str) -> Result<(), String> {
        self.fire(HostCommand::AddNode {
            name: name.into(),
            profile: profile.into(),
            initial_prompt: None,
        });
        Ok(())
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

    /// Send a message to a named agent.
    SendTo { name: String, message: String },

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
}

/// Read-only snapshot of host + topology state, returned by GetStatus.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HostStatus {
    /// Names of all registered (running) agents.
    pub agents: Vec<String>,
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
    /// Current termination spec (as JSON string).
    pub termination: String,
}
