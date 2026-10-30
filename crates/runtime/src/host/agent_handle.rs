//! Per-agent control struct: channels + task handle for one running agent.

use std::sync::Arc;

use agentik_core::agent::InternalEvent;
use agentik_core::memory::KmsAgentHandle;
use agentik_core::profile::AgentKind;
use agentik_sdk::model::Model;
use agentik_sdk::types::{AgentEvent, ContentBlock};
use arc_swap::ArcSwapOption;
use tokio_util::sync::CancellationToken;

/// Control handle for one running agent.
///
/// Cheap to move; owns the event receiver and cancellation token. When
/// dropped, the agent's tokio task is **not** automatically killed — call
/// [`shutdown`](Self::shutdown) explicitly.
pub struct AgentHandle {
    pub agent_id: uuid::Uuid,
    pub path: agentik_types::AgentPath,
    /// Agent kind this agent was instantiated from. Sub-agents reuse it
    /// unless they explicitly request the other role.
    pub kind: AgentKind,
    pub(super) internal_tx: tokio::sync::mpsc::UnboundedSender<InternalEvent>,
    pub(super) event_rx: tokio::sync::mpsc::UnboundedReceiver<AgentEvent>,
    pub(super) agent_task:
        tokio::task::JoinHandle<std::result::Result<(), agentik_core::supervise::TaskPanic>>,
    pub(super) cancel_token: CancellationToken,
    pub(super) model: Arc<ArcSwapOption<Model>>,
    pub(super) memory: Arc<KmsAgentHandle>,
}

impl AgentHandle {
    /// Convenience: short name (last path segment).
    pub fn name(&self) -> &str {
        self.path.name()
    }
}

impl AgentHandle {
    /// TUI user input; the caller has already displayed the message locally.
    pub fn send_message(&self, text: String) {
        self.send_message_from(text, true);
    }

    pub(super) fn send_message_from(&self, text: String, from_user: bool) {
        let _ = self.internal_tx.send(InternalEvent::MessageInject {
            content: vec![ContentBlock::Text { text }],
            from_user,
            delegation_id: None,
        });
    }

    /// Send a tracked request and carry the delegation ID through the target
    /// agent's turn lifecycle.
    pub fn send_delegation(&self, text: String, delegation_id: uuid::Uuid) {
        let _ = self.internal_tx.send(InternalEvent::MessageInject {
            content: vec![ContentBlock::Text { text }],
            from_user: false,
            delegation_id: Some(delegation_id),
        });
    }

    pub fn cancel(&mut self) {
        self.cancel_token.cancel();
        let new_token = CancellationToken::new();
        let _ = self
            .internal_tx
            .send(InternalEvent::ResetCancelToken(new_token.clone()));
        self.cancel_token = new_token;
    }

    /// Trigger compaction on the agent's active session.
    pub fn compact(&self) {
        let _ = self.internal_tx.send(InternalEvent::Compact);
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
    ///
    /// If the agent task panicked at any point (detected via `catch_unwind`),
    /// the panic details are logged here as an additional safety net.
    pub async fn join(self) {
        match self.agent_task.await {
            Ok(Ok(())) => {} // clean exit
            Ok(Err(panic)) => {
                tracing::error!(
                    agent = %self.path,
                    panic_task = %panic.task,
                    panic_msg = %panic.msg,
                    "agent task had panicked before join"
                );
            }
            Err(join_err) => {
                tracing::error!(
                    agent = %self.path,
                    error = %join_err,
                    "agent task join failed (panic or cancellation)"
                );
            }
        }
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

    /// Update this agent's runtime settings in place.
    ///
    /// The same backend Arc is held by `AgentShared`, so prompt injection,
    /// memory tools, and background consolidation observe the update without
    /// rebuilding the agent or dropping its sessions.
    pub fn set_runtime_config(&self, config: agentik_core::AgentRuntimeConfig) {
        self.memory.set_runtime_config(config);
    }

    pub fn runtime_config(&self) -> agentik_core::AgentRuntimeConfig {
        self.memory.runtime_config()
    }

    // ── Context metadata ──────────────────────────────────

    /// Request a list of all sessions. The reply arrives as
    /// `AgentEvent::SessionList` on the event channel — with one context
    /// per agent it carries at most one entry (the agent's sole context).
    pub fn list_sessions(&self) {
        let _ = self.internal_tx.send(InternalEvent::ListSessions);
    }
}
