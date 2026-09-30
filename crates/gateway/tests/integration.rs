//! End-to-end tests for the gateway daemon: a real axum server on an
//! ephemeral port, a mock model, and a real `GatewayClient` + SSE pump.
//! No network beyond loopback, no API keys — the same reliability
//! keystone the headless tests established, now over the wire.

use std::time::Duration;

use eventsource_stream::Eventsource;
use futures::StreamExt;
use gateway::client::{EventPump, GatewayClient, GatewayFrame};
use gateway::proto::HostEventView;
use gateway::testing::{TEST_TIMEOUT, start_mock_gateway};

/// Collect pump frames until `pred` matches; fails on timeout.
async fn wait_for_frame<F>(
    rx: &mut tokio::sync::mpsc::UnboundedReceiver<GatewayFrame>,
    pred: F,
) -> GatewayFrame
where
    F: Fn(&GatewayFrame) -> bool,
{
    tokio::time::timeout(TEST_TIMEOUT, async {
        loop {
            let frame = rx.recv().await.expect("pump channel open");
            if pred(&frame) {
                return frame;
            }
        }
    })
    .await
    .expect("frame arrived within timeout")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn spawn_chat_turn_and_shutdown_over_the_wire() {
    let gateway_daemon = start_mock_gateway("The answer is 2.").await;
    let client = gateway_daemon.client();

    // Hydration first.
    let state = client.state().await.unwrap();
    assert!(state.agents.is_empty());
    assert!(!state.profiles.is_empty());

    // Start the event pump.
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    EventPump::spawn(client.clone(), state.last_seq, tx);

    // Spawn an agent from the first profile.
    let profile = state.profiles.first().unwrap().clone();
    let path = client
        .spawn_agent("worker", "/root", &profile, None)
        .await
        .unwrap();
    assert!(
        path.ends_with("worker"),
        "spawn returns the full path: {path}"
    );

    // Registration arrives as a host frame.
    let registered = wait_for_frame(&mut rx, |f| {
        matches!(
            f,
            GatewayFrame::Sequenced {
                inner: gateway::client::FrameInner::Host(HostEventView::AgentRegistered { .. }),
                ..
            }
        )
    })
    .await;
    let GatewayFrame::Sequenced {
        seq: registered_seq,
        ..
    } = registered
    else {
        unreachable!()
    };

    // Deliver a user message; the scripted mock model completes the turn.
    client.deliver_message(&path, "What is 1+1?").await.unwrap();
    let completed = wait_for_frame(&mut rx, |f| {
        matches!(
            f,
            GatewayFrame::Sequenced {
                inner: gateway::client::FrameInner::Agent {
                    event: agentik_types::AgentEvent::TurnCompleted { .. },
                    ..
                },
                ..
            }
        )
    })
    .await;

    // Every frame carries an increasing seq — the hub's total order.
    let GatewayFrame::Sequenced {
        seq: completed_seq, ..
    } = completed
    else {
        unreachable!()
    };
    assert!(completed_seq > registered_seq);

    // /state now lists the live agent, and the driver's session cache has
    // the agent's sessions (requested at registration, refreshed when the
    // turn started on a session the cache hadn't seen).
    let refreshed = tokio::time::timeout(TEST_TIMEOUT, async {
        loop {
            let snapshot = client.state().await.unwrap();
            if snapshot.agents.iter().any(|a| a.path == path)
                && snapshot
                    .sessions
                    .get(&path)
                    .is_some_and(|list| !list.is_empty())
            {
                return snapshot;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("agent + sessions visible in /state");
    assert!(!refreshed.sessions[&path].is_empty());

    let records = client.list_storage_agents().await.unwrap();
    let record = records
        .iter()
        .find(|record| record.name == path)
        .expect("worker agent record persisted");
    let stored_sessions = client.list_stored_sessions(record.id).await.unwrap();
    assert!(
        stored_sessions.iter().any(|session| session.active),
        "stored session list includes the live session"
    );

    let initial_config = client.agent_runtime_config(&path).await.unwrap();
    assert!(!initial_config.effective.use_memory);
    assert!(!initial_config.effective.generate_memory);
    let saved_config = client
        .set_agent_runtime_config(
            &path,
            agentik_core::AgentRuntimeOverrides {
                use_memory: Some(true),
                generate_memory: Some(false),
            },
        )
        .await
        .unwrap();
    assert!(saved_config.effective.use_memory);
    assert!(!saved_config.effective.generate_memory);
    let refreshed_config = client.agent_runtime_config(&path).await.unwrap();
    assert!(refreshed_config.effective.use_memory);
    assert!(!refreshed_config.effective.generate_memory);

    // Replay: reconnect with Last-Event-ID below the terminal frame and
    // observe the replayed TurnCompleted again (dedup is the client's
    // job — the raw stream must still deliver it).
    let replay_response = client.connect_events(registered_seq).await.unwrap();
    let mut replay = replay_response.bytes_stream().eventsource();
    let saw_turn_completed = tokio::time::timeout(TEST_TIMEOUT, async {
        loop {
            match replay.next().await {
                Some(Ok(event)) => {
                    if event.event == "agent" && event.data.contains("\"TurnCompleted\"") {
                        return true;
                    }
                }
                _ => return false,
            }
        }
    })
    .await
    .expect("replay delivers the missed frame");
    assert!(saw_turn_completed);

    // Graceful shutdown over the wire.
    client.shutdown_gateway().await.unwrap();
    gateway_daemon.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn bearer_token_gates_the_api() {
    let gateway_daemon = start_mock_gateway("x").await;

    // Without the token: 401.
    let unauthenticated = GatewayClient::new(&gateway_daemon.addr.to_string(), None).unwrap();
    let err = unauthenticated.state().await.unwrap_err();
    assert_eq!(err.status().unwrap_or(0), 401);

    // With the token: fine.
    gateway_daemon.client().state().await.unwrap();
    gateway_daemon.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn swagger_docs_expose_the_gateway_api() {
    let gateway_daemon = start_mock_gateway("x").await;
    let base = format!("http://{}", gateway_daemon.addr);
    let http = reqwest::Client::new();

    let openapi: serde_json::Value = http
        .get(format!("{base}/api/v1/api-docs/openapi.json"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(openapi["info"]["title"], "Autonomics Gateway API");
    assert_eq!(openapi["paths"].as_object().unwrap().len(), 34);
    let expected_paths = [
        "/api/v1/gateway/status",
        "/api/v1/gateway/shutdown",
        "/api/v1/state",
        "/api/v1/agents",
        "/api/v1/agents/{name}/messages",
        "/api/v1/agents/{name}/cancel",
        "/api/v1/agents/{name}/compact",
        "/api/v1/agents/{name}/shutdown",
        "/api/v1/agents/{name}/model",
        "/api/v1/agents/{name}/config",
        "/api/v1/agents/{name}/dag",
        "/api/v1/agents/{name}/sessions",
        "/api/v1/agents/{name}/sessions/{id}/activate",
        "/api/v1/agents/{name}/sessions/{id}/close",
        "/api/v1/agents/{name}/sessions/{id}/title",
        "/api/v1/agents/{agent_id}/sessions/{session_id}/history",
        "/api/v1/agents/{agent_id}/plan",
        "/api/v1/storage/agents",
        "/api/v1/storage/agents/{id}",
        "/api/v1/storage/agents/{id}/sessions",
        "/api/v1/model-config",
        "/api/v1/model-config/provider",
        "/api/v1/model-config/active-model",
        "/api/v1/model-config/chatgpt/login",
        "/api/v1/model-config/chatgpt/refresh",
        "/api/v1/model-config/providers/{name}/catalog",
        "/api/v1/settings",
        "/api/v1/events",
        "/api/v1/skills/evolution",
        "/api/v1/skills/evolution/trigger",
        "/api/v1/skills/evolution/proposals",
        "/api/v1/skills/evolution/proposals/{name}/approve",
        "/api/v1/skills/evolution/proposals/{name}/reject",
    ];
    for path in expected_paths {
        assert!(
            openapi["paths"].as_object().unwrap().contains_key(path),
            "missing OpenAPI path: {path}"
        );
    }
    assert_eq!(
        openapi["components"]["securitySchemes"]["bearer_auth"]["type"],
        "http"
    );
    assert!(
        openapi["paths"]
            .as_object()
            .unwrap()
            .values()
            .flat_map(serde_json::Value::as_object)
            .flat_map(|operations| operations.values())
            .all(|operation| operation["security"][0]["bearer_auth"].is_array())
    );

    let swagger = http.get(format!("{base}/swagger-ui")).send().await.unwrap();
    assert!(swagger.status().is_success());
    assert!(swagger.text().await.unwrap().contains("swagger-ui"));

    // Documentation is metadata-only; operational endpoints stay guarded.
    let unauthorized = http
        .get(format!("{base}/api/v1/state"))
        .send()
        .await
        .unwrap();
    assert_eq!(unauthorized.status(), 401);

    gateway_daemon.stop().await;
}

// ── skill evolution endpoints ─────────────────────────────────────────

/// Poll an async predicate until it holds or the test timeout fires —
/// the evolution service distills on its own event path, so "who
/// wrote the proposal" (worker or explicit trigger) is intentionally
/// racy and the test only asserts observable state.
async fn eventually<F, Fut>(mut pred: F)
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    tokio::time::timeout(TEST_TIMEOUT, async {
        loop {
            if pred().await {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("condition eventually held");
}

#[tokio::test]
async fn skill_evolution_status_trigger_approve_reject_roundtrip() {
    let gw = start_mock_gateway("ok").await;
    let client = gw.client();
    let manager = gw.infra.skills.clone();

    // Baseline: fresh isolated state, service armed, gate closed.
    let status = client.skill_evolution_status().await.unwrap();
    assert!(status.service_enabled);
    assert!(!status.auto_approve);
    assert_eq!(status.proposals_pending, 0);

    // Seed one anchor with three distinct fixes.
    for body in ["cast the column", "pass format=csv", "validate header"] {
        manager
            .record_observation(skills::ObservationInput {
                kind: skills::ObservationKind::Failure,
                source: skills::ObservationSource::Agent,
                summary: "sql boom".into(),
                body: body.into(),
                node_kind: Some("sql".into()),
                error: Some("syntax error near 42".into()),
            })
            .unwrap();
    }

    // Either the event-driven worker or our explicit trigger writes
    // the proposal; both paths produce one pending row.
    let _ = client.trigger_skill_evolution(None).await.unwrap();
    eventually(|| {
        let client = client.clone();
        async move {
            client
                .skill_proposals()
                .await
                .map(|p| p.iter().any(|x| x.status == "pending"))
                .unwrap_or(false)
        }
    })
    .await;

    // Approve over the wire; generation advances, library grows.
    let generation_before = client.skill_evolution_status().await.unwrap().generation;
    let pending_name = client
        .skill_proposals()
        .await
        .unwrap()
        .into_iter()
        .find(|p| p.status == "pending")
        .unwrap()
        .name;
    let outcome = client.approve_skill_proposal(&pending_name).await.unwrap();
    assert!(outcome.destination.contains(&pending_name));
    eventually(|| {
        let client = client.clone();
        let before = generation_before;
        async move {
            client
                .skill_evolution_status()
                .await
                .map(|s| s.generation > before)
                .unwrap_or(false)
        }
    })
    .await;
    let status = client.skill_evolution_status().await.unwrap();
    assert_eq!(status.proposals_pending, 0);
    assert_eq!(status.proposals_approved, 1);
    assert!(status.skills_auto >= 1);

    // Second anchor: trigger with the auto gate, verify the report
    // says it approved, then exercise reject on a third.
    for body in ["io a", "io b", "io c"] {
        manager
            .record_observation(skills::ObservationInput {
                kind: skills::ObservationKind::Failure,
                source: skills::ObservationSource::Agent,
                summary: "io crash".into(),
                body: body.into(),
                node_kind: Some("file_to_dataframe".into()),
                error: Some("bad schema at 7".into()),
            })
            .unwrap();
    }
    for body in ["x a", "x b", "x c"] {
        manager
            .record_observation(skills::ObservationInput {
                kind: skills::ObservationKind::Failure,
                source: skills::ObservationSource::Agent,
                summary: "third anchor".into(),
                body: body.into(),
                node_kind: Some("echo".into()),
                error: Some("echo fail 9".into()),
            })
            .unwrap();
    }
    eventually(|| {
        let client = client.clone();
        async move {
            client
                .skill_proposals()
                .await
                .map(|p| p.iter().filter(|x| x.status == "pending").count() >= 2)
                .unwrap_or(false)
        }
    })
    .await;

    let proposals = client.skill_proposals().await.unwrap();
    let to_reject = proposals
        .iter()
        .find(|p| p.status == "pending" && p.name != pending_name)
        .unwrap()
        .name
        .clone();
    let rejected = client.reject_skill_proposal(&to_reject).await.unwrap();
    assert_eq!(rejected.status, "rejected");

    let status = client.skill_evolution_status().await.unwrap();
    assert_eq!(status.proposals_rejected, 1);
}
