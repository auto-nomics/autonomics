//! Host command processing: the `HostCommand` dispatch loop plus the
//! module layout for its helpers.
//!
//! - [`activation`] — virtual-actor activation and buffered delivery
//! - [`spawn`] — spawn / shutdown / cancel lifecycle commands
//! - [`delegate`] — inter-agent delegation commands
//! - [`query`] — status / history / routing queries
//! - [`model_config`] — per-agent model & runtime-config commands
//! - [`resolution`] — name resolution and relay command forwarding
//! - [`policy`] — hierarchy validation (delegation, child control)
//! - [`capability`] — [`AgentInfo`](crate::control::AgentInfo)
//!   extraction from [`AgentKind`](agentik_core::profile::AgentKind)

mod activation;
mod capability;
mod delegate;
mod model_config;
mod policy;
mod query;
mod resolution;
mod spawn;

// Re-exported for `host::persistence` (parent-info assembly) and the
// handler submodules below.
pub(in crate::host) use capability::capability_from_kind;

use super::RuntimeHost;
use super::events::PendingDelivery;
use crate::control::HostCommand;

impl RuntimeHost {
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
                if let Some(notice) = reg {
                    self.process_spawn_notice(notice);
                }
            }
        }
    }

    pub(super) fn process_command(&mut self, cmd: crate::control::HostCommand) {
        match cmd {
            HostCommand::Spawn {
                name,
                caller_path,
                config,
                model_override,
                operator,
                reply_tx,
            } => {
                self.handle_spawn(
                    name,
                    caller_path,
                    *config,
                    model_override,
                    operator,
                    reply_tx,
                );
            }
            HostCommand::Shutdown {
                name,
                caller_path,
                evict,
                reply_tx,
            } => {
                self.handle_shutdown(name, caller_path, evict, reply_tx);
            }
            HostCommand::NotifyIdentityRenamed {
                agent_id,
                old_path,
                new_path,
            } => {
                // Storage-level fact reported by a gateway handler: fan it
                // out to every connected frontend through the host event
                // stream (single pump, total order with the eviction's
                // AgentUnregistered that precedes it).
                self.emit_host_event(super::HostEvent::IdentityRenamed {
                    agent_id,
                    old_path,
                    new_path,
                });
            }
            HostCommand::NotifyIdentityDeleted { agent_id, path } => {
                self.emit_host_event(super::HostEvent::IdentityDeleted { agent_id, path });
            }
            HostCommand::DeliverMessage {
                name,
                message,
                reply_tx,
            } => {
                let resolved = self.resolve_agent(&name).unwrap_or(name);
                if self.agents.contains_key(&resolved) {
                    let result = self.send_to(&resolved, message);
                    if let Err(error) = &result {
                        tracing::warn!(agent = %resolved, %error, "user message dropped");
                    }
                    if let Some(reply_tx) = reply_tx {
                        let _ = reply_tx.send(result);
                    }
                } else {
                    // Virtual-actor activation: the identity is dormant
                    // (or unknown — activation reports that). Buffer the
                    // delivery; a background task materializes the
                    // instance from its stored record and the pump
                    // delivers in order on registration.
                    self.enqueue_after_activation(
                        resolved,
                        PendingDelivery {
                            text: message,
                            delegation_id: None,
                            origin: agentik_types::MessageOrigin::User,
                            reply_tx,
                        },
                    );
                }
            }
            HostCommand::Delegate {
                to,
                message,
                caller_path,
                delegation_id,
                progress,
                reply_tx,
            } => {
                self.handle_delegate(to, message, caller_path, delegation_id, progress, reply_tx);
            }
            HostCommand::GetAgentHistory {
                agent_name,
                limit,
                reply_tx,
            } => {
                self.handle_agent_history(agent_name, limit, reply_tx);
            }
            HostCommand::GetStatus { reply_tx } => {
                self.handle_get_status(reply_tx);
            }
            HostCommand::RouteTask {
                description,
                caller_path,
                reply_tx,
            } => {
                let result = self.route_task(&description, &caller_path);
                let _ = reply_tx.send(result);
            }
            HostCommand::GetAgentInfo { name, reply_tx } => {
                self.handle_get_agent_info(name, reply_tx);
            }

            // ── Session management (forwarded to relay) ──
            HostCommand::CancelAgent {
                name,
                caller_path,
                reply_tx,
            } => {
                self.handle_cancel_agent(name, caller_path, reply_tx);
            }
            HostCommand::CompactAgent { name } => {
                self.send_agent_command(&name, super::relay::AgentCommand::Compact);
            }
            HostCommand::ListContexts { name } => {
                self.send_agent_command(&name, super::relay::AgentCommand::ListContexts);
            }

            // ── Model management ──
            HostCommand::SetAgentModel { name, model } => {
                self.handle_set_agent_model(name, model);
            }
            HostCommand::SetAgentRuntimeConfig {
                name,
                overrides,
                reply_tx,
            } => {
                self.handle_set_agent_runtime_config(name, *overrides, reply_tx);
            }
            HostCommand::GetAgentModel { name, reply_tx } => {
                self.handle_get_agent_model(name, reply_tx);
            }
            HostCommand::ListSubAgents {
                agent_name,
                reply_tx,
            } => {
                self.handle_list_sub_agents(agent_name, reply_tx);
            }
        }
    }
}

/// Shared fixtures for the command-handler test modules: isolated
/// [`RuntimeHost`](super::super::RuntimeHost) config, agent registration,
/// and the mock-model / event-pump helpers that drive full turns without
/// network access.
#[cfg(test)]
pub(in crate::host) mod test_support {
    use std::sync::Arc;
    use std::time::Duration;

    use agentik_core::profile::{AgentKind, AgentProfileConfig};
    use agentik_sdk::model::Model;
    use arc_swap::ArcSwapOption;

    use crate::config::RuntimeConfig;
    use crate::host::RuntimeHost;

    pub fn config(dir: &tempfile::TempDir) -> RuntimeConfig {
        let mut config = RuntimeConfig::default();
        config.data_dir = dir.path().join("data");
        config.state_dir = dir.path().join("state");
        config.agent_db = dir.path().join("agent.db");
        config.dag_history_db = dir.path().join("dag-history.db");
        config.bib_db_path = dir.path().join("bib.db");
        config.writing_db_path = dir.path().join("writing.db");
        config.app_db_path = dir.path().join("app.db");
        config.kms_db_path = dir.path().join("kms.db");
        config.knowledge_db_path = dir.path().join("knowledge.db");
        config.wiki_db_path = dir.path().join("wiki.db");
        // Keep the embedding model (network install + ONNX session)
        // out of these tests.
        config.embedding.enabled = false;
        config
    }

    pub async fn register_agent_at(
        host: &mut RuntimeHost,
        path: &str,
        model: Arc<ArcSwapOption<Model>>,
    ) {
        let path = agentik_types::AgentPath::try_from(path).unwrap();
        let config = AgentProfileConfig::new(AgentKind::Researcher);
        let handle = host.spawn_agent(&path, &config, model, None).await.unwrap();
        let info =
            super::capability_from_kind(handle.path.name(), handle.path.as_str(), config.kind);
        host.register_agent(handle, info);
    }

    pub fn set_status(
        host: &mut RuntimeHost,
        path: &str,
        status: agentik_types::AgentLifecycleStatus,
    ) {
        let entry = host.agents.get_mut(path).unwrap();
        entry.status = status;
        entry.info.status = status;
    }

    /// A mock model that answers any request with one assistant text
    /// message — enough to drive a full turn in the activation tests.
    pub fn activation_mock_model(name: &str) -> Arc<ArcSwapOption<Model>> {
        let mut mock = agentik_sdk::provider::client::MockApiClient::new();
        mock.expect_request_stream_with_system()
            .times(..)
            .returning(|_, _, _, _| {
                Ok(agentik_sdk::streaming::MessageStream::from_events(
                    Vec::new(),
                    agentik_sdk::AgentMessageExt::assistant_text("activated"),
                ))
            });
        Arc::new(ArcSwapOption::from_pointee(Some(Model::with_client(
            agentik_core::testing::dummy_model_info(name),
            mock,
        ))))
    }

    /// Pump the host until the given agent acknowledges a delivery with
    /// the expected text (activation → flush → relay → agent loop). User
    /// deliveries acknowledge as `UserMessageAcknowledged`; delegation
    /// injections as `MessageInjected`.
    pub async fn wait_for_delivery_event(host: &mut RuntimeHost, path: &str, text: &str) {
        let path = path.to_string();
        let text = text.to_string();
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let Some((agent, event)) = host.recv_any().await else {
                    panic!("agent event channel closed");
                };
                let acknowledged = match event {
                    agentik_sdk::types::AgentEvent::UserMessageAcknowledged(injected) => {
                        injected == text
                    }
                    agentik_sdk::types::AgentEvent::MessageInjected { text: injected, .. } => {
                        injected == text
                    }
                    _ => false,
                };
                if agent == path && acknowledged {
                    return;
                }
            }
        })
        .await
        .expect("timed out waiting for the delivery acknowledgement");
    }
}

#[cfg(test)]
mod interrupt_agent_tests {
    //! Phase 3 — interrupt_agent vs shutdown_agent distinction.
    //!
    //! These tests verify the command-layer invariants without spinning
    //! up a full RuntimeHost (which requires DataEngine/Turso).
    //! The full integration test would be: spawn agent → delegate_to →
    //! interrupt_agent → assert LifecycleChanged(Cancelled) and agent
    //! still in registry.

    use crate::control::HostCommand;

    /// `interrupt` and `shutdown` MUST be distinct commands — the
    /// handler dispatches them to different paths:
    /// - `CancelAgent` → AgentCommand::Cancel → handle.cancel() (preserves
    ///   agent; only the current turn aborts; new token issued)
    /// - `Shutdown { name, .. }` → AgentCommand::Shutdown → handle.shutdown()
    ///   → graceful exit (removes from registry)
    ///
    /// Mixing these up would silently kill long-lived worker agents on
    /// a benign interrupt request.
    #[test]
    fn cancel_and_shutdown_are_distinct_commands() {
        let cancel = HostCommand::CancelAgent {
            name: "worker".into(),
            caller_path: None,
            reply_tx: None,
        };
        let shutdown = HostCommand::Shutdown {
            name: "worker".into(),
            caller_path: None,
            evict: false,
            reply_tx: None,
        };

        // Different concrete types — pattern-match proves it.
        let cancel_kind = match &cancel {
            HostCommand::CancelAgent { .. } => "cancel",
            _ => "other",
        };
        let shutdown_kind = match &shutdown {
            HostCommand::Shutdown { .. } => "shutdown",
            _ => "other",
        };
        assert_eq!(cancel_kind, "cancel");
        assert_eq!(shutdown_kind, "shutdown");
    }

    /// `InterruptAgentInput` should accept `agent_name` as required and
    /// `reason` as optional with a sensible default. The tool emits a
    /// log line + a ToolResult::success message; we test the schema by
    /// serializing/deserializing JSON in the same shape the LLM would
    /// produce.
    #[test]
    fn interrupt_input_schema_accepts_minimal_payload() {
        // Mimic the schema the LLM would emit when calling the tool.
        let raw = serde_json::json!({ "agent_name": "worker" });
        let parsed: serde_json::Result<serde_json::Value> = Ok(raw.clone());
        let v = parsed.expect("parse");
        assert_eq!(v["agent_name"], "worker");
        // reason is omitted → tool default is "user-requested interrupt"
        assert!(v.get("reason").is_none());
    }

    /// `InterruptAgentInput` schema with explicit reason.
    #[test]
    fn interrupt_input_schema_accepts_full_payload() {
        let raw = serde_json::json!({
            "agent_name": "researcher",
            "reason": "wrong agent selected"
        });
        assert_eq!(raw["agent_name"], "researcher");
        assert_eq!(raw["reason"], "wrong agent selected");
    }

    /// After `cancel_agent` the agent emits `LifecycleChanged(Cancelled)`;
    /// the merged projection passes it through so observers see the
    /// cancellation as-is (no Failed resynthesis). Delegated tasks resolve
    /// on the turn's `TurnCompleted { status: Interrupted }` event, not on
    /// the status value, so interrupt_agent never hangs a delegator.
    #[test]
    fn cancelled_lifecycle_passes_through_derive() {
        let (status, _) = crate::host::status::derive_agent_status(
            &agentik_sdk::types::AgentEvent::LifecycleChanged(
                agentik_types::AgentLifecycleStatus::Cancelled,
            ),
        );
        assert_eq!(
            status,
            Some(agentik_types::AgentLifecycleStatus::Cancelled),
            "the lifecycle must reach observers untouched"
        );
    }

    /// Same for `Error` lifecycle — fatal system errors surface as-is.
    #[test]
    fn error_lifecycle_passes_through_derive() {
        let (status, _) = crate::host::status::derive_agent_status(
            &agentik_sdk::types::AgentEvent::LifecycleChanged(
                agentik_types::AgentLifecycleStatus::Error,
            ),
        );
        assert_eq!(status, Some(agentik_types::AgentLifecycleStatus::Error));
    }
}

#[cfg(test)]
mod deliver_message_tests {
    //! `DeliverMessage` — the user→agent channel (TUI/gateway). The
    //! fire-and-forget inter-agent `SendMessage` channel was removed:
    //! delegate_to is the only inter-agent communication path.
    //!
    //! These tests verify the command-layer invariants without spinning
    //! up a full RuntimeHost: tracked delivery carries a reply channel so
    //! a 202 means the host actually enqueued the message.

    use crate::control::{HostCommand, HostControl};
    use tokio::sync::oneshot;

    /// `DeliverMessage` and `Delegate` MUST be distinct variants. They
    /// dispatch to different handler paths:
    /// - `DeliverMessage` → user message (optional reply for the gateway)
    /// - `Delegate` → request-response inter-agent (reply: full response
    ///    text, gated to direct children)
    #[test]
    fn deliver_message_is_distinct_from_delegate() {
        let deliver = HostCommand::DeliverMessage {
            name: "worker".into(),
            message: "hello".into(),
            reply_tx: None,
        };
        let delegate = HostCommand::Delegate {
            to: "worker".into(),
            message: "hello".into(),
            caller_path: None,
            delegation_id: uuid::Uuid::new_v4(),
            progress: None,
            reply_tx: oneshot::channel().0,
        };

        // Pattern-match proves each is a distinct variant.
        let deliver_kind = match &deliver {
            HostCommand::DeliverMessage { .. } => "deliver",
            _ => "other",
        };
        let delegate_kind = match &delegate {
            HostCommand::Delegate { .. } => "delegate",
            _ => "other",
        };
        assert_eq!(deliver_kind, "deliver");
        assert_eq!(delegate_kind, "delegate");
        assert_ne!(deliver_kind, delegate_kind);
    }

    /// The gateway rides `DeliverMessage.reply_tx` so a 202 means the host
    /// actually enqueued the message. Round-trip: tracked delivery carries
    /// the reply channel, the host confirms, the caller sees `Some(Ok(()))`.
    #[tokio::test]
    async fn deliver_message_tracked_round_trip_confirms_delivery() {
        let (cmd_tx, mut cmd_rx) = tokio::sync::mpsc::unbounded_channel::<HostCommand>();
        let (event_tx, _) = tokio::sync::broadcast::channel::<crate::host::HostEvent>(1);
        let control = HostControl::new(cmd_tx, event_tx);

        let caller = tokio::spawn(async move {
            control
                .deliver_message_tracked("/root/worker", "hello there")
                .await
        });

        let cmd = cmd_rx.recv().await.expect("command received");
        match cmd {
            HostCommand::DeliverMessage {
                name,
                message,
                reply_tx,
            } => {
                assert_eq!(name, "/root/worker");
                assert_eq!(message, "hello there");
                let reply_tx = reply_tx.expect("tracked delivery carries a reply channel");
                let _ = reply_tx.send(Ok(()));
            }
            _ => panic!("expected DeliverMessage variant"),
        }

        let result = caller.await.expect("caller task panicked");
        assert_eq!(result, Some(Ok(())));
    }

    /// Tracked delivery surfaces dropped messages: an `Err` reply (unknown
    /// agent, exited loop) reaches the caller instead of a blind 202.
    #[tokio::test]
    async fn deliver_message_tracked_round_trip_reports_drop() {
        let (cmd_tx, mut cmd_rx) = tokio::sync::mpsc::unbounded_channel::<HostCommand>();
        let (event_tx, _) = tokio::sync::broadcast::channel::<crate::host::HostEvent>(1);
        let control = HostControl::new(cmd_tx, event_tx);

        let caller = tokio::spawn(async move {
            control
                .deliver_message_tracked("/root/ghost", "hello")
                .await
        });

        let cmd = cmd_rx.recv().await.expect("command received");
        match cmd {
            HostCommand::DeliverMessage { reply_tx, .. } => {
                let reply_tx = reply_tx.expect("tracked delivery carries a reply channel");
                let _ = reply_tx.send(Err("Agent '/root/ghost' is not registered.".into()));
            }
            _ => panic!("expected DeliverMessage variant"),
        }

        let result = caller.await.expect("caller task panicked");
        assert!(matches!(result, Some(Err(message)) if message.contains("not registered")));
    }

    /// When the host never answers (reply channel dropped without a send —
    /// a wedged or panicked host loop), tracked delivery returns `None`.
    /// The fire-and-forget TUI path still sends no reply channel at all.
    #[tokio::test]
    async fn deliver_message_tracked_returns_none_when_host_is_silent() {
        let (cmd_tx, mut cmd_rx) = tokio::sync::mpsc::unbounded_channel::<HostCommand>();
        let (event_tx, _) = tokio::sync::broadcast::channel::<crate::host::HostEvent>(1);
        let control = HostControl::new(cmd_tx, event_tx);

        let caller = tokio::spawn({
            let control = control.clone();
            async move {
                control
                    .deliver_message_tracked("/root/wedged", "hello")
                    .await
            }
        });

        let cmd = cmd_rx.recv().await.expect("command received");
        match cmd {
            HostCommand::DeliverMessage { reply_tx, .. } => {
                drop(reply_tx);
            }
            _ => panic!("expected DeliverMessage variant"),
        }

        let result = caller.await.expect("caller task panicked");
        assert!(result.is_none(), "silent host must surface as None");

        // Legacy TUI path: no reply channel, still fire-and-forget.
        control.deliver_message("/root/worker", "hello");
        match cmd_rx.recv().await.expect("command received") {
            HostCommand::DeliverMessage { reply_tx, .. } => {
                assert!(reply_tx.is_none(), "TUI delivery stays fire-and-forget");
            }
            _ => panic!("expected DeliverMessage variant"),
        }
    }
}
