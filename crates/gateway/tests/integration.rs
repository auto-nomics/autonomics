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
