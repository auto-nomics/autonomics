//! Durable projections of the in-memory host state: delegation records
//! and per-turn telemetry. All writes are fire-and-forget background
//! tasks — the in-memory registry is authoritative for the live runtime.
//!
//! There is intentionally NO persisted agent layout: which agents are
//! open is frontend view state, and under virtual-actor mode instances
//! materialize from their stored records on demand (`activate_identity`
//! in `commands.rs`). The `agents` table alone is the identity source.

use agentik_core::storage::{AgentDelegationRecord, AgentTurnRecord};

use super::RuntimeHost;

impl RuntimeHost {
    pub(super) fn persist_delegation(&self, delegation_id: uuid::Uuid) {
        let Some(snapshot) = self.ledger.get(&delegation_id) else {
            return;
        };
        let storage = self.infra.storage.clone();
        let record = AgentDelegationRecord {
            delegation_id: snapshot.delegation_id,
            // Storage boundary: the DB record stays string-typed; the
            // typed `AgentPath` projects down here.
            caller_path: snapshot
                .caller_path
                .as_ref()
                .map(|path| path.as_str().to_string()),
            target_path: snapshot.target_path.as_str().to_string(),
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
        agentik_core::supervise::spawn_safe_on(
            &self.infra.runtime_handle,
            &format!("persist_turn_start::{turn_id}"),
            async move {
                // The executing context is not carried by the event (one
                // context per agent makes it derivable) — resolve it from
                // the `agent_contexts` binding, which is written before
                // any turn of the owning agent can start.
                let Ok(Some(session_id)) = storage.agent_context_id(agent_id).await else {
                    tracing::debug!(%turn_id, "turn start not persisted: no bound context");
                    return;
                };
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
            agentik_types::TurnExecutionStatus::Pause => "paused",
        };
        agentik_core::supervise::spawn_safe_on(
            &self.infra.runtime_handle,
            &format!("persist_turn_finish::{turn_id}"),
            async move {
                let Ok(Some(session_id)) = storage.agent_context_id(agent_id).await else {
                    tracing::debug!(%turn_id, "turn finish not persisted: no bound context");
                    return;
                };
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
}

fn unix_epoch_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|value| value.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod agent_persistence_tests {
    use super::*;
    use crate::HostEvent;
    use crate::config::RuntimeConfig;
    use agentik_core::profile::{AgentKind, AgentProfileConfig};
    use agentik_sdk::model::Model;
    use agentik_sdk::types::{AgentEvent, ContentBlock};
    use arc_swap::ArcSwapOption;
    use std::sync::Arc;
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
        config.kms_db_path = dir.path().join("kms.db");
        config.knowledge_db_path = dir.path().join("knowledge.db");
        config.wiki_db_path = dir.path().join("wiki.db");
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

    /// A mock model that answers any request with one assistant text
    /// message; `times(..)` so drop verification tolerates idle instances.
    fn answering_model(name: &str) -> Arc<ArcSwapOption<Model>> {
        let mut mock = agentik_sdk::provider::client::MockApiClient::new();
        mock.expect_request_stream_with_system()
            .times(..)
            .returning(|_, _, _, _| {
                Ok(agentik_sdk::streaming::MessageStream::from_events(
                    Vec::new(),
                    agentik_sdk::AgentMessageExt::assistant_text("delegated work done"),
                ))
            });
        Arc::new(ArcSwapOption::from_pointee(Some(Model::with_client(
            agentik_core::testing::dummy_model_info(name),
            mock,
        ))))
    }

    /// Delegation survives a daemon restart WITHOUT any persisted layout:
    /// after restart the registry starts empty, and the first message to
    /// the child identity activates it from its stored record — same
    /// agent ID, same sole context, WAL intact.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn child_agent_inter_agent_messages_survive_restart() {
        let dir = tempfile::tempdir().unwrap();
        let mut host = RuntimeHost::open(&config(&dir)).await.unwrap();
        let model = answering_model("child-delegation-restart");
        host.set_model(model.clone());
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
        let delegate_control = control.clone();
        let delivery_path = path.clone();
        let caller_path = parent_path.as_str().to_string();
        let delivery = tokio::spawn(async move {
            delegate_control
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

        // Keep pumping host events until the delegated turn's trailing
        // `Done`: delegation resolution keys on Done (TurnCompleted only
        // records the outcome), so the reply fires in that same pass.
        timeout(Duration::from_secs(10), async {
            loop {
                let Some((event_path, event)) = host.recv_any().await else {
                    panic!("agent event channel closed");
                };
                if event_path == path.as_str() && matches!(event, AgentEvent::Done) {
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

        // host1 sanity: the live incarnation answers list_contexts.
        control.list_contexts(path.as_str());
        host.recv_and_process_command().await;
        timeout(Duration::from_secs(3), async {
            loop {
                let Some((event_path, event)) = host.recv_any().await else {
                    panic!("host1 event channel closed");
                };
                if event_path == path.as_str() && matches!(event, AgentEvent::ContextList { .. }) {
                    break;
                }
            }
        })
        .await
        .expect("host1 must answer list_contexts");

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

        // ── Restart: no layout restore — an empty registry — and the
        // first message activates the identity from its record. ──
        let mut host = RuntimeHost::open(&config(&dir)).await.unwrap();
        assert!(
            host.agent_names().is_empty(),
            "restart must start with an empty registry (virtual actor)"
        );
        host.set_model(answering_model("layout-restart-test"));

        let control = host.control();
        let delivery_path = path.clone();
        let delivery = tokio::spawn(async move {
            control
                .deliver_message_tracked(delivery_path.as_str(), "ping")
                .await
        });
        host.recv_and_process_command().await; // DeliverMessage → activation task
        host.recv_and_process_command().await; // SpawnNotice::Activated → register + flush
        timeout(Duration::from_secs(10), async {
            loop {
                let Some((event_path, event)) = host.recv_any().await else {
                    panic!("host agent event channel closed");
                };
                if event_path == path.as_str()
                    && let AgentEvent::UserMessageAcknowledged(text) = event
                    && text == "ping"
                {
                    return;
                }
            }
        })
        .await
        .expect("activated agent must acknowledge the delivery");
        assert_eq!(delivery.await.unwrap(), Some(Ok(())));
        assert!(host.contains_agent(path.as_str()));
        assert_eq!(
            host.agents
                .get(path.as_str())
                .and_then(|entry| entry.info.agent_id),
            Some(agent_id),
            "activation must reuse the stored identity"
        );

        // Wait for the turn to finish before asking for the session list:
        // run_session owns the internal-event channel mid-turn, and a
        // ListContexts arriving then is swallowed by its loop (pre-existing
        // behavior — the daemon driver also fires only after TurnCompleted).
        timeout(Duration::from_secs(10), async {
            loop {
                let Some((_, event)) = host.recv_any().await else {
                    panic!("host agent event channel closed");
                };
                if matches!(event, AgentEvent::Done)
                    || matches!(
                        event,
                        AgentEvent::LifecycleChanged(agentik_types::AgentLifecycleStatus::Idle,)
                    )
                {
                    return;
                }
            }
        })
        .await
        .expect("activated turn must complete");

        // The activated agent re-bound its sole context: the persisted
        // session is still the only one, with the same ID.
        let control = host.control();
        control.list_contexts(path.as_str());
        host.recv_and_process_command().await; // pump the ListContexts command
        let outcome = timeout(Duration::from_secs(10), async {
            let mut seen: Vec<String> = Vec::new();
            loop {
                let Some((event_path, event)) = host.recv_any().await else {
                    panic!("host agent event channel closed");
                };
                if event_path == path.as_str()
                    && let AgentEvent::ContextList { context } = event
                {
                    return context;
                }
                seen.push(format!("{event_path}::{event:?}"));
                if seen.len() > 40 {
                    panic!(
                        "no ContextList; events seen: {seen:?}; live={} ",
                        host.contains_agent(path.as_str())
                    );
                }
            }
        })
        .await;
        let sessions = match outcome {
            Ok(sessions) => sessions,
            Err(_) => {
                let mut buffered = Vec::new();
                while let Some((event_path, event)) = host.try_recv_any() {
                    buffered.push(format!("{event_path}::{event:?}"));
                }
                let status = host.agents.get(path.as_str()).map(|entry| {
                    (
                        entry.status,
                        entry.background_pending,
                        entry.last_event.clone(),
                    )
                });
                let probe_storage = host.storage().clone();
                let probe_agent_id = agent_id;
                let probe = tokio::time::timeout(
                    Duration::from_secs(3),
                    probe_storage.list_session_records(probe_agent_id),
                )
                .await;
                let probe_result = match probe {
                    Err(_) => "list_session_records TIMED OUT (storage lock held?)".to_string(),
                    Ok(Err(e)) => format!("list_session_records errored: {e}"),
                    Ok(Ok(records)) => format!("list_session_records ok: {} rows", records.len()),
                };
                panic!(
                    "no ContextList; {probe_result}; live={} agents={:?}; status={status:?}; buffered after timeout: {buffered:#?}",
                    host.contains_agent(path.as_str()),
                    host.agent_names()
                );
            }
        };
        let sole = sessions.expect("one sole context restored");
        assert_eq!(sole.id, session_id);
    }
}
