//! Durable projections of the in-memory host state: the persisted agent
//! graph, the atomic layout snapshot, delegation records, and per-turn
//! telemetry. All writes are fire-and-forget background tasks — the
//! in-memory registry is authoritative for the live runtime.

use std::sync::Arc;

use agentik_core::profile::AgentProfileConfig;
use agentik_core::storage::{
    AgentDelegationRecord, AgentLayoutSnapshot, AgentTurnRecord, PersistedAgentGraph,
};
use agentik_sdk::model::Model;
use arc_swap::ArcSwapOption;

use crate::error::Result;

use super::{HostEvent, RuntimeHost, unix_epoch_ms};

impl RuntimeHost {
    /// Persist the exact set and status of currently registered agents.
    pub(super) fn agent_layout_snapshot(&self, revision: u64) -> AgentLayoutSnapshot {
        let updated_at = unix_epoch_ms();
        let mut agents = Vec::with_capacity(self.agents.len());

        for entry in self.agents.values() {
            let Some(agent_id) = entry.info.agent_id else {
                continue;
            };
            let Ok(status_json) = serde_json::to_string(&entry.status) else {
                continue;
            };
            let parent_path = agentik_types::AgentPath::try_from(entry.info.path.as_str())
                .ok()
                .and_then(|path| path.parent().map(|parent| parent.as_str().to_string()));
            agents.push((
                entry.layout_order,
                PersistedAgentGraph {
                    path: entry.info.path.clone(),
                    parent_path,
                    profile_path: entry.kind.name().to_string(),
                    agent_id,
                    status_json,
                    last_event: entry.last_event.clone(),
                    created_at: entry.layout_created_at,
                    updated_at,
                },
            ));
        }
        agents.sort_by_key(|(order, _)| *order);
        let agents = agents
            .into_iter()
            .map(|(_, entry)| entry)
            .collect::<Vec<_>>();

        AgentLayoutSnapshot { revision, agents }
    }

    /// Queue an atomic layout snapshot write without blocking the host pump.
    pub(super) fn persist_current_agent_layout(&mut self) {
        let revision = self.next_layout_revision;
        self.next_layout_revision = self.next_layout_revision.saturating_add(1);
        let snapshot = self.agent_layout_snapshot(revision);
        let storage = self.infra.storage.clone();
        agentik_core::supervise::spawn_safe_on_drop(
            &self.infra.runtime_handle,
            "persist_current_agent_layout",
            async move {
                if let Err(error) = storage.save_agent_layout(&snapshot).await {
                    tracing::warn!(%error, "failed to persist agent layout");
                }
            },
        );
    }

    /// Spawn a background task that upserts the agent's graph row in storage.
    ///
    /// Caller has already inserted the in-memory entry. This mirrors the
    /// registration into the `agent_graph` table so the dashboard can
    /// reconstruct the topology across process restarts. Errors are
    /// logged at WARN — persistence is best-effort.
    pub(super) fn persist_upsert_agent_graph(
        &self,
        path: &agentik_types::AgentPath,
        agent_id: uuid::Uuid,
        profile_path: &str,
        info: &crate::control::AgentInfo,
    ) {
        let storage = self.infra.storage.clone();
        let path_str = path.as_str().to_string();
        let parent_path = path.parent().map(|p| p.as_str().to_string());
        let profile_path = profile_path.to_string();
        let status_json = match serde_json::to_string(&info.status) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(agent = %path_str, error = %e, "skip persistence: status serialization failed");
                return;
            }
        };
        let last_event = info.last_event.clone();

        agentik_core::supervise::spawn_safe_on_drop(
            &self.infra.runtime_handle,
            &format!("persist_upsert_agent_graph::{path_str}"),
            async move {
                // Wall-clock millis since the unix epoch. std::time::SystemTime
                // is the only reliable source here (the runtime crate doesn't
                // depend on chrono at runtime — it's a dev-dep only).
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_millis() as i64)
                    .unwrap_or(0);
                let entry = PersistedAgentGraph {
                    path: path_str.clone(),
                    parent_path,
                    profile_path,
                    agent_id,
                    status_json,
                    last_event,
                    created_at: now,
                    updated_at: now,
                };
                if let Err(e) = storage.upsert_agent_graph_entry(entry).await {
                    tracing::warn!(
                        agent = %path_str,
                        error = %e,
                        "failed to persist agent graph entry (non-fatal)"
                    );
                }
            },
        );
    }

    /// Spawn a background task that updates the agent's persisted status.
    /// Mirrors [`Self::observe_status`] to durable storage. Fire-and-forget.
    pub(super) fn persist_agent_status(
        &self,
        path: &str,
        status: &crate::control::AgentStatus,
        last_event: &Option<String>,
    ) {
        let storage = self.infra.storage.clone();
        let path_str = path.to_string();
        let status_json = match serde_json::to_string(status) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(agent = %path_str, error = %e, "skip status persistence: serialization failed");
                return;
            }
        };
        let last_event = last_event.clone();

        agentik_core::supervise::spawn_safe_on_drop(
            &self.infra.runtime_handle,
            &format!("persist_agent_status::{path_str}"),
            async move {
                if let Err(e) = storage
                    .update_agent_graph_status(&path_str, &status_json, last_event.as_deref())
                    .await
                {
                    tracing::warn!(
                        agent = %path_str,
                        error = %e,
                        "failed to persist agent status (non-fatal)"
                    );
                }
            },
        );
    }

    pub(super) fn persist_delegation(&self, delegation_id: uuid::Uuid) {
        let Some(snapshot) = self
            .delegations
            .get(&delegation_id)
            .map(|record| record.snapshot.clone())
        else {
            return;
        };
        let storage = self.infra.storage.clone();
        let record = AgentDelegationRecord {
            delegation_id: snapshot.delegation_id,
            caller_path: snapshot.caller_path,
            target_path: snapshot.target_path,
            task: snapshot.task,
            status: snapshot.status.as_str().to_string(),
            turn_id: snapshot.turn_id,
            session_id: snapshot.session_id,
            response: snapshot.response,
            created_at: snapshot.created_at,
            updated_at: snapshot.updated_at,
        };
        agentik_core::supervise::spawn_safe_on(
            &self.infra.runtime_handle,
            &format!("persist_delegation::{delegation_id}"),
            async move {
                if let Err(error) = storage.upsert_agent_delegation(record).await {
                    tracing::warn!(%delegation_id, %error, "failed to persist delegation");
                }
            },
        );
    }

    pub(super) fn persist_turn_start(
        &self,
        agent_path: &str,
        turn_id: uuid::Uuid,
        session_id: uuid::Uuid,
        delegation_id: Option<uuid::Uuid>,
    ) {
        let Some(agent_id) = self
            .agents
            .get(agent_path)
            .and_then(|entry| entry.info.agent_id)
        else {
            return;
        };
        let storage = self.infra.storage.clone();
        let record = AgentTurnRecord {
            turn_id,
            agent_id,
            session_id,
            delegation_id,
            status: "running".to_string(),
            started_at: unix_epoch_ms(),
            completed_at: None,
            telemetry: None,
        };
        agentik_core::supervise::spawn_safe_on(
            &self.infra.runtime_handle,
            &format!("persist_turn_start::{turn_id}"),
            async move {
                if let Err(error) = storage.start_agent_turn(record).await {
                    tracing::warn!(%turn_id, %error, "failed to persist turn start");
                }
            },
        );
    }

    pub(super) fn persist_turn_finish(
        &self,
        agent_path: &str,
        telemetry: agentik_types::TurnTelemetry,
        turn_id: uuid::Uuid,
        session_id: uuid::Uuid,
        delegation_id: Option<uuid::Uuid>,
        status: agentik_types::TurnExecutionStatus,
    ) {
        let Some(agent_id) = self
            .agents
            .get(agent_path)
            .and_then(|entry| entry.info.agent_id)
        else {
            return;
        };
        let storage = self.infra.storage.clone();
        let status = match status {
            agentik_types::TurnExecutionStatus::Completed => "completed",
            agentik_types::TurnExecutionStatus::Interrupted => "interrupted",
            agentik_types::TurnExecutionStatus::Failed => "failed",
        };
        agentik_core::supervise::spawn_safe_on(
            &self.infra.runtime_handle,
            &format!("persist_turn_finish::{turn_id}"),
            async move {
                let record = AgentTurnRecord {
                    turn_id,
                    agent_id,
                    session_id,
                    delegation_id,
                    status: status.to_string(),
                    started_at: unix_epoch_ms(),
                    completed_at: Some(unix_epoch_ms()),
                    telemetry: Some(telemetry),
                };
                if let Err(error) = storage.finish_agent_turn(record).await {
                    tracing::warn!(%turn_id, %error, "failed to persist turn completion");
                }
            },
        );
    }

    /// Spawn a background task that removes the agent's persisted graph row.
    /// Called on shutdown. Fire-and-forget.
    pub(super) fn persist_remove_agent_graph(&self, path: &str) {
        let storage = self.infra.storage.clone();
        let path_str = path.to_string();

        agentik_core::supervise::spawn_safe_on_drop(
            &self.infra.runtime_handle,
            &format!("persist_remove_agent_graph::{path_str}"),
            async move {
                if let Err(e) = storage.remove_agent_graph_entry(&path_str).await {
                    tracing::warn!(
                        agent = %path_str,
                        error = %e,
                        "failed to remove persisted agent graph entry (non-fatal)"
                    );
                }
            },
        );
    }

    /// Restore the multi-agent layout left behind by the previous daemon.
    ///
    /// The current-layout snapshot records the exact set of agents that were
    /// open when the daemon stopped, along with their hierarchical paths and
    /// stable IDs. The record in `agents` carries the per-agent
    /// configuration used by that incarnation (kind, model preference,
    /// runtime overrides), parsed tolerantly so rows written by the old
    /// profile registry still restore. Individual corrupt or unresolvable
    /// entries are skipped so one stale row cannot prevent the daemon and
    /// the remaining agents from starting.
    pub async fn restore_persisted_agents<F>(
        &mut self,
        global_model: Arc<ArcSwapOption<Model>>,
        mut resolve_model: F,
    ) -> Result<usize>
    where
        F: AsyncFnMut(&str) -> std::result::Result<Model, String>,
    {
        let entries = self
            .infra
            .storage
            .load_agent_layout()
            .await?
            .map(|snapshot| snapshot.agents)
            .unwrap_or_default();

        let mut restored = 0;
        for entry in entries {
            let path = match agentik_types::AgentPath::try_from(entry.path.as_str()) {
                Ok(path) => path,
                Err(error) => {
                    tracing::warn!(
                        path = %entry.path,
                        error = %error,
                        "skipping persisted agent with invalid path"
                    );
                    continue;
                }
            };

            let record = match self
                .infra
                .storage
                .get_agent(entry.agent_id)
                .await
                .ok()
                .flatten()
            {
                Some(record) => record,
                None => match self
                    .infra
                    .storage
                    .get_agent_by_name(entry.path.as_str())
                    .await
                {
                    Ok(Some(record)) => record,
                    Ok(None) => {
                        tracing::warn!(
                            path = %entry.path,
                            agent_id = %entry.agent_id,
                            "skipping persisted agent without an agents record"
                        );
                        continue;
                    }
                    Err(error) => {
                        tracing::warn!(
                            path = %entry.path,
                            error = %error,
                            "failed to load persisted agent record"
                        );
                        continue;
                    }
                },
            };

            let config = AgentProfileConfig::from_json(&record.config_json);

            let model_override = match config.preferred_model.as_deref() {
                Some(spec) => match resolve_model(spec).await {
                    Ok(model) => Some(model),
                    Err(error) => {
                        tracing::warn!(
                            path = %entry.path,
                            model = spec,
                            error = %error,
                            "skipping persisted agent because its preferred model is unavailable"
                        );
                        continue;
                    }
                },
                None => None,
            };

            if model_override.is_none() && global_model.load_full().is_none() {
                tracing::warn!(
                    path = %entry.path,
                    "skipping persisted agent because no default model is configured"
                );
                continue;
            }

            match self
                .spawn_agent(&path, &config, global_model.clone(), model_override)
                .await
            {
                Ok(handle) => {
                    let agent_id = handle.agent_id;
                    let mut info = super::commands::capability_from_kind(
                        handle.path.name(),
                        handle.path.as_str(),
                        config.kind,
                    );
                    info.agent_id = Some(agent_id);
                    self.register_agent(handle, info.clone());
                    self.emit_host_event(HostEvent::AgentRegistered {
                        path: path.clone(),
                        info,
                    });
                    restored += 1;
                    tracing::info!(
                        agent = %entry.path,
                        agent_id = %entry.agent_id,
                        "restored persisted agent layout entry"
                    );
                }
                Err(error) => {
                    tracing::warn!(
                        path = %entry.path,
                        error = %error,
                        "failed to restore persisted agent"
                    );
                }
            }
        }

        Ok(restored)
    }
}

#[cfg(test)]
mod agent_persistence_tests {
    use super::*;
    use crate::config::RuntimeConfig;
    use agentik_core::profile::AgentKind;
    use agentik_core::profile::AgentProfileConfig;
    use agentik_sdk::types::{AgentEvent, ContentBlock};
    use std::time::Duration;
    use tokio::time::timeout;

    fn config(dir: &tempfile::TempDir) -> RuntimeConfig {
        let mut config = RuntimeConfig::default();
        config.data_dir = dir.path().join("data");
        config.state_dir = dir.path().join("state");
        config.agent_db = dir.path().join("agent.db");
        config.dag_history_db = dir.path().join("dag-history.db");
        config.bib_db_path = dir.path().join("bib.db");
        config.writing_db_path = dir.path().join("writing.db");
        config.app_db_path = dir.path().join("app.db");
        config.knowledge_db_path = dir.path().join("knowledge.db");
        // The parent (/root/researcher) is a root-level agent; without this
        // it would run the startup memory pipeline (default on) against an
        // empty model slot in the first host and add noise to the event
        // stream this test drives.
        config.use_memory = false;
        config.generate_memory = false;
        // Keep the embedding model (network install + ONNX session)
        // out of these tests.
        config.embedding.enabled = false;
        config
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn child_agent_inter_agent_messages_survive_restart() {
        let dir = tempfile::tempdir().unwrap();
        let mut host = RuntimeHost::open(&config(&dir)).await.unwrap();
        // Install a mock model up front so the delegated child turn
        // actually completes — the delegation reply only resolves on
        // TurnCompleted. The expectation matches ANY request (no `.with`)
        // and answers with a plain assistant text message.
        let mut mock = agentik_sdk::provider::client::MockApiClient::new();
        mock.expect_request_stream_with_system()
            .times(1..)
            .returning(|_, _, _, _| {
                Ok(agentik_sdk::streaming::MessageStream::from_events(
                    Vec::new(),
                    agentik_sdk::AgentMessageExt::assistant_text("delegated work done"),
                ))
            });
        let model: Arc<ArcSwapOption<Model>> =
            Arc::new(ArcSwapOption::from_pointee(Some(Model::with_client(
                agentik_core::testing::dummy_model_info("child-delegation-restart"),
                mock,
            ))));
        host.set_model(model);
        let path = agentik_types::AgentPath::root()
            .join("researcher")
            .unwrap()
            .join("worker")
            .unwrap();
        let spawn_config = AgentProfileConfig::new(AgentKind::Researcher);
        let spawn_path = path.clone();
        let spawn_control = host.control();
        let spawn = tokio::spawn(async move {
            let parent = spawn_path.parent().unwrap();
            spawn_control
                .spawn_with_config(spawn_path.name(), &parent, spawn_config, None)
                .await
        });
        host.recv_and_process_command().await;
        host.recv_and_process_command().await;
        let registration = host.recv_event().await.unwrap();
        let HostEvent::AgentRegistered {
            path: registered_path,
            info,
        } = registration
        else {
            panic!("expected agent registration event");
        };
        assert_eq!(registered_path, path);
        let agent_id = info.agent_id.expect("registered event carries agent ID");
        assert_eq!(spawn.await.unwrap().unwrap(), path.as_str());

        // Delegation needs a live caller — register the parent agent
        // (/root/researcher) so it can delegate to its direct child.
        let parent_path = path.parent().unwrap();
        let parent_config = AgentProfileConfig::new(AgentKind::Researcher);
        let parent_handle = host
            .spawn_agent(
                &parent_path,
                &parent_config,
                Arc::new(ArcSwapOption::from_pointee(None)),
                None,
            )
            .await
            .unwrap();
        let parent_info = super::super::commands::capability_from_kind(
            parent_handle.path.name(),
            parent_handle.path.as_str(),
            parent_config.kind,
        );
        host.register_agent(parent_handle, parent_info);

        let control = host.control();
        let delivery_path = path.clone();
        let caller_path = parent_path.as_str().to_string();
        let delivery = tokio::spawn(async move {
            control
                .delegate_tracked(
                    delivery_path.as_str(),
                    "persist child message",
                    Some(caller_path),
                    uuid::Uuid::new_v4(),
                    None,
                )
                .await
                .expect("host command channel should remain open")
        });
        host.recv_and_process_command().await;

        let injected = timeout(Duration::from_secs(5), async {
            loop {
                let Some((_, event)) = host.recv_any().await else {
                    panic!("agent event channel closed");
                };
                if matches!(event, AgentEvent::MessageInjected(_)) {
                    return event;
                }
            }
        })
        .await
        .expect("expected injected message event");
        assert!(
            matches!(injected, AgentEvent::MessageInjected(text) if text == "persist child message")
        );

        // Keep pumping host events until the delegated turn completes; the
        // delegation reply resolves inside the same event-processing pass
        // (process_tagged resolves reply_tx on TurnCompleted), so awaiting
        // the delivery task afterwards only needs a scheduler tick.
        timeout(Duration::from_secs(10), async {
            loop {
                let Some((event_path, event)) = host.recv_any().await else {
                    panic!("agent event channel closed");
                };
                if event_path == path.as_str() && matches!(event, AgentEvent::TurnCompleted { .. })
                {
                    return;
                }
            }
        })
        .await
        .expect("expected delegated turn to complete");
        let delegated_response = timeout(Duration::from_secs(5), delivery)
            .await
            .expect("delegation reply must resolve after the child's turn completes")
            .expect("delegation task must not panic");
        assert!(
            !delegated_response.is_empty(),
            "delegation must return the child's response"
        );

        // The WAL write is asynchronous and is not tied to event delivery.
        tokio::time::sleep(Duration::from_millis(100)).await;
        host.shutdown_all_agents_and_wait().await;
        let storage = host.infra().storage.clone();
        let records = storage.list_session_records(agent_id).await.unwrap();
        assert_eq!(records.len(), 1, "child session row must be persisted");

        let state = agentik_core::storage::restore_session_state(
            storage.as_ref(),
            agent_id,
            records[0].session_id,
        )
        .await
        .unwrap();
        assert!(
            state
                .messages
                .iter()
                .any(|message| message.content.iter().any(|block| {
                    matches!(block, ContentBlock::Text { text } if text == "persist child message")
                })),
            "inter-agent message must be in the child session WAL"
        );
        let session_id = records[0].session_id;
        drop(storage);
        drop(host);

        let mut host = RuntimeHost::open(&config(&dir)).await.unwrap();
        let model: Arc<ArcSwapOption<Model>> = Arc::new(ArcSwapOption::from_pointee(Some(
            agentik_core::testing::get_mock_model("layout-restart-test"),
        )));
        let restored = host
            .restore_persisted_agents(model, async |spec| {
                panic!("unexpected model preference `{spec}`")
            })
            .await
            .unwrap();
        assert_eq!(
            restored, 2,
            "persisted parent + child layout entries must both be restored"
        );
        assert!(host.agent_names().contains(&path.as_str()));

        assert_eq!(
            host.agents
                .get(path.as_str())
                .and_then(|entry| entry.info.agent_id),
            Some(agent_id),
            "same child path must restore its ID"
        );

        let control = host.control();
        control.list_sessions(path.as_str());
        host.recv_and_process_command().await;
        let sessions = timeout(Duration::from_secs(2), async {
            loop {
                let Some((event_path, event)) = host.recv_any().await else {
                    panic!("host agent event channel closed");
                };
                assert_eq!(event_path, path.as_str());
                if let AgentEvent::SessionList { sessions } = event {
                    return sessions;
                }
            }
        })
        .await
        .expect("expected restored session list");
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].id, session_id);
    }
}
