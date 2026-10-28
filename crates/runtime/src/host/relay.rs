//! Per-agent relay: the registry entry, the command/event bridge task, and
//! the message-transport + shutdown paths that talk to registered agents.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use agentik_core::memory::KmsAgentHandle;
use agentik_core::profile::AgentKind;
use agentik_sdk::model::Model;
use arc_swap::ArcSwapOption;
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use tokio::task::JoinHandle;

use super::{AgentHandle, HostEvent, RuntimeHost, TaggedEvent, unix_epoch_ms};

/// Commands sent to a per-agent relay task.
pub(super) enum AgentCommand {
    Message {
        text: String,
        delegation_id: Option<uuid::Uuid>,
        from_user: bool,
    },
    Shutdown,
    Cancel,
    ListSessions,
    CreateSession {
        title: Option<String>,
        fork_from: Option<uuid::Uuid>,
        fork_message_id: Option<String>,
    },
    SwitchSession(uuid::Uuid),
    CloseSession(uuid::Uuid),
    RenameSession {
        id: uuid::Uuid,
        title: String,
    },
    SetModel(Model),
    Compact,
}

/// Internal entry for one registered agent.
pub(super) struct AgentEntry {
    pub(super) cmd_tx: UnboundedSender<AgentCommand>,
    pub(super) _relay_task: JoinHandle<std::result::Result<(), agentik_core::supervise::TaskPanic>>,
    /// Capability metadata for routing and discovery.
    ///
    /// `status` and `last_event` mirror the live runtime fields — the
    /// host writes to both this struct and `info` so that
    /// `GetAgentInfo` / `GetStatus` (which serialize `AgentInfo`) see the
    /// same value that `update_agent_status` last set.
    pub(super) info: crate::control::AgentInfo,
    /// Agent kind used to instantiate this agent; needed to rebuild the
    /// layout without re-reading `agents.config_json`.
    pub(super) kind: AgentKind,
    /// Layout insertion position assigned by [`RuntimeHost`].
    pub(super) layout_order: u64,
    /// First time this incarnation entered the daemon layout.
    pub(super) layout_created_at: i64,
    /// Live runtime status. Updated on every observed [`agentik_sdk::types::AgentEvent`].
    /// Phase-1 deliverable — surfaces "what is agent X doing?" without
    /// requiring the agent to expose anything new.
    pub(super) status: crate::control::AgentStatus,
    /// Most recent event summary (tool name on AwaitingTool, error
    /// message on Failed, etc.). `None` until the first event lands.
    pub(super) last_event: Option<String>,
    /// Shared model slot — same Arc as the relay's AgentHandle.
    /// Allows querying and hot-swapping the model without direct
    /// access to the moved AgentHandle.
    pub(super) model: Arc<ArcSwapOption<Model>>,
    /// Shared memory backend — same Arc as the agent's runtime.
    pub(super) memory: Arc<KmsAgentHandle>,
    /// Set when the host enqueues an inbound message but the target agent has
    /// not yet emitted `TurnStarted`. This closes the Idle-status race where
    /// several peer messages could queue before the runtime status caught up.
    pub(super) inbound_pending: Arc<AtomicBool>,
}

impl RuntimeHost {
    // ── Agent registration + multiplexed transport ─────────

    /// Register an [`AgentHandle`] with the host's internal multiplexer.
    ///
    /// After registration, use [`send_to`](Self::send_to) for message
    /// delivery and [`recv_any`](Self::recv_any) for unified event polling.
    /// The host takes ownership of the handle — do not use it directly
    /// after registration.
    ///
    /// Returns the enriched info (with `agent_id` and the model fields
    /// filled from the live slot) for callers to broadcast in
    /// [`HostEvent::AgentRegistered`].
    pub fn register_agent(&mut self, handle: AgentHandle, info: crate::control::AgentInfo) -> crate::control::AgentInfo {
        let path = handle.path.clone();
        let kind = handle.kind;
        let agent_id = handle.agent_id;
        let relay_name = path.as_str().to_string();
        let model = handle.model.clone(); // Clone Arc before moving handle
        let memory = Arc::clone(&handle.memory);
        let mut info = info;
        info.agent_id = Some(agent_id);
        fill_model_fields(&mut info, &model);
        let event_tx = self.event_tx.clone();
        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel::<AgentCommand>();
        let layout_order = self.next_layout_order;
        self.next_layout_order = self.next_layout_order.saturating_add(1);
        let layout_created_at = unix_epoch_ms();

        let relay_task = agentik_core::supervise::spawn_safe_on(
            &self.infra.runtime_handle,
            &format!("relay::{relay_name}"),
            async move {
                relay_loop(handle, cmd_rx, event_tx, relay_name).await;
            },
        );

        self.agents.insert(
            path.as_str().to_string(),
            AgentEntry {
                cmd_tx,
                _relay_task: relay_task,
                status: info.status.clone(),
                last_event: info.last_event.clone(),
                info: info.clone(),
                kind,
                layout_order,
                layout_created_at,
                model,
                memory,
                inbound_pending: Arc::new(AtomicBool::new(false)),
            },
        );

        // ── Phase 4: persist agent graph entry ──
        // Fire-and-forget: registration semantics are owned by the in-memory
        // registry; the dashboard's persistence is a read-side projection that
        // can tolerate eventual consistency. Failure here is logged but does
        // not abort the spawn flow.
        self.persist_upsert_agent_graph(&path, agent_id, kind.name(), &info);
        self.persist_current_agent_layout();
        info
    }

    /// Send a message to a named agent (via the relay task).
    ///
    /// `name` should already be a resolved full path. Callers that receive
    /// user/LLM-provided names should call [`resolve_agent`](Self::resolve_agent)
    /// first.
    ///
    /// Returns `Err` when the message was dropped — the agent is not in the
    /// live registry (e.g. the registry was emptied by a restart) or its
    /// command loop has exited. Historically both cases failed silently,
    /// which let callers acknowledge messages that never reached anyone.
    pub fn send_to(&self, name: &str, message: String) -> std::result::Result<(), String> {
        self.send_to_from_user(name, message, true)
    }

    fn send_to_from_user(
        &self,
        name: &str,
        message: String,
        from_user: bool,
    ) -> std::result::Result<(), String> {
        if !self.agents.contains_key(name) {
            return Err(format!(
                "Agent '{name}' is not registered. The live registry is empty until \
                 agents are spawned and resets on daemon restart; re-spawn or recover \
                 the agent before delivering messages."
            ));
        }
        if !self.enqueue_agent_message(name, message, None, from_user) {
            return Err(format!(
                "Agent '{name}' is not accepting commands (its loop has exited)."
            ));
        }
        Ok(())
    }

    pub(super) fn enqueue_agent_message(
        &self,
        name: &str,
        message: String,
        delegation_id: Option<uuid::Uuid>,
        from_user: bool,
    ) -> bool {
        if let Some(entry) = self.agents.get(name) {
            let sent = entry
                .cmd_tx
                .send(AgentCommand::Message {
                    text: message,
                    delegation_id,
                    from_user,
                })
                .is_ok();
            if sent {
                entry.inbound_pending.store(true, Ordering::Release);
            }
            sent
        } else {
            false
        }
    }

    /// Query the model info for a named agent (for TUI rendering).
    /// The string is the provider-precise `provider:model` spec.
    pub fn agent_model_info(&self, name: &str) -> Option<(String, u64)> {
        self.agents.get(name).and_then(|e| {
            e.model
                .load_full()
                .as_deref()
                .map(|m| (m.model_spec(), m.model_info.context_length))
        })
    }

    /// Clone an entry's capability info with the model fields refreshed
    /// from the live slot. `GetStatus` / `GetAgentInfo` serialize through
    /// this so a later hot-swap (per-agent override or the shared daemon
    /// default) is reflected without re-registering.
    pub(super) fn snapshot_info(&self, entry: &AgentEntry) -> crate::control::AgentInfo {
        let mut info = entry.info.clone();
        fill_model_fields(&mut info, &entry.model);
        info
    }

    /// Notify listeners that an agent has been shut down.
    /// Called from `shutdown_agent` / `shutdown_all_agents`.
    fn notify_unregistered(&self, name: &str) {
        self.emit_host_event(HostEvent::AgentUnregistered {
            path: name.to_string(),
        });
    }

    /// Shut down a named agent and remove it from the registry.
    pub fn shutdown_agent(&mut self, name: &str) {
        self.fail_pending_delegations(name, "target agent shut down before completion");
        if let Some(entry) = self.agents.remove(name) {
            let _ = entry.cmd_tx.send(AgentCommand::Shutdown);
        }
        self.notify_unregistered(name);
        // Phase 4: remove the persisted graph row so the dashboard doesn't
        // resurrect a stale entry on the next process start.
        self.persist_remove_agent_graph(name);
        self.persist_current_agent_layout();
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
        let shutdown_revision = self.next_layout_revision;
        self.next_layout_revision = self.next_layout_revision.saturating_add(1);
        let shutdown_snapshot = self.agent_layout_snapshot(shutdown_revision);
        let names: Vec<String> = self.agents.keys().cloned().collect();
        for name in &names {
            self.fail_pending_delegations(name, "target agent shut down before completion");
        }
        let mut relay_tasks = Vec::new();
        for (_, entry) in self.agents.drain() {
            let _ = entry.cmd_tx.send(AgentCommand::Shutdown);
            relay_tasks.push(entry._relay_task);
        }
        for name in names {
            self.notify_unregistered(&name);
        }
        if let Err(error) = self
            .infra
            .storage
            .save_agent_layout(&shutdown_snapshot)
            .await
        {
            tracing::warn!(%error, "failed to persist final agent layout during shutdown");
        }
        // Wait for all relay tasks to finish. Each relay loop calls
        // `handle.join().await` before exiting, which in turn waits for
        // the agent's `run()` to complete its session-pause shutdown.
        for task in relay_tasks {
            match task.await {
                Ok(Ok(())) => {}
                Ok(Err(panic)) => {
                    tracing::error!(
                        target: "spawn_safe",
                        task = %panic.task,
                        panic = %panic.msg,
                        "relay task panicked during shutdown"
                    );
                }
                Err(join_err) => {
                    tracing::error!(error = %join_err, "relay task join error during shutdown");
                }
            }
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
        for name in &names {
            self.fail_pending_delegations(name, "target agent shut down before completion");
        }
        for (_, entry) in self.agents.drain() {
            let _ = entry.cmd_tx.send(AgentCommand::Shutdown);
        }
        for name in names {
            self.notify_unregistered(&name);
        }
    }
}

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

            cmd = cmd_rx.recv() => match cmd {
                Some(AgentCommand::Message {
                    text,
                    delegation_id,
                    from_user,
                }) => match delegation_id {
                    Some(id) => handle.send_delegation(text, id),
                    None => handle.send_message_from(text, from_user),
                }
                Some(AgentCommand::Cancel) => {
                    handle.cancel();
                }
                Some(AgentCommand::ListSessions) => {
                    handle.list_sessions();
                }
                Some(AgentCommand::CreateSession { title, fork_from, fork_message_id }) => {
                    let _ = handle.create_session(title, fork_from, fork_message_id);
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
                Some(AgentCommand::Compact) => {
                    handle.compact();
                }
                Some(AgentCommand::Shutdown) | None => {
                    handle.shutdown();
                    break;
                }
            },

            // Events are checked AFTER commands so that a streaming
            // event flood cannot starve Cancel / Shutdown.  Commands are
            // infrequent, so prioritising them adds negligible latency
            // to event forwarding while guaranteeing prompt interrupts.
            event = handle.recv_event() => match event {
                Some(ev) => {
                    if event_tx.send((name.clone(), ev)).is_err() {
                        break;
                    }
                }
                None => break,
            },
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

/// Overlay the live model spec onto an [`AgentInfo`](crate::control::AgentInfo).
/// Reads through the shared `ArcSwapOption`, so agents that inherited the
/// daemon-wide default slot report the current default, not the one at
/// registration time. Clears both fields when the slot is empty.
fn fill_model_fields(
    info: &mut crate::control::AgentInfo,
    model: &Arc<ArcSwapOption<Model>>,
) {
    match model.load_full().as_deref() {
        Some(model) => {
            info.model = Some(model.model_spec());
            info.context_length = Some(model.model_info.context_length);
        }
        None => {
            info.model = None;
            info.context_length = None;
        }
    }
}
