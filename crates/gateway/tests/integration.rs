//! End-to-end tests for the gateway daemon: a real axum server on an
//! ephemeral port, a mock model, and a real `GatewayClient` + SSE pump.
//! No network beyond loopback, no API keys — the same reliability
//! keystone the headless tests established, now over the wire.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use agentik_sdk::model::{Model, ModelInfo};
use agentik_sdk::provider::client::MockApiClient;
use agentik_sdk::streaming::MessageStream;
use agentik_types::errors::AnthropicError;
use agentik_types::messages::{ContentBlock, Message, Role, StopReason};
use agentik_types::streaming::{
    ContentBlockDelta, MessageDelta, MessageDeltaUsage, MessageStreamEvent,
};
use arc_swap::ArcSwapOption;
use gateway::client::{EventPump, GatewayClient, GatewayFrame};
use gateway::driver::{self, SessionCache};
use gateway::hub::EventHub;
use gateway::proto::HostEventView;
use gateway::server::{GatewayState, router_with_bib};
use runtime::{RuntimeConfig, RuntimeHost};
use tokio_util::sync::CancellationToken;

const TEST_TOKEN: &str = "test-token";
const TEST_TIMEOUT: Duration = Duration::from_secs(30);

struct TestGateway {
    addr: std::net::SocketAddr,
    server: tui_http::HttpServerHandle,
    shutdown: CancellationToken,
    _dir: tempfile::TempDir,
}

impl TestGateway {
    fn client(&self) -> GatewayClient {
        GatewayClient::new(&self.addr.to_string(), Some(TEST_TOKEN)).unwrap()
    }

    async fn stop(self) {
        self.shutdown.cancel();
        let _ = self.server.shutdown().await;
    }
}

/// Redirect EVERY persistent path under the test temp dir — the same
/// isolation the headless tests use (`RuntimeConfig::default()` bakes
/// absolute `~/.autonomics` paths).
fn isolate_runtime_paths(config: &mut RuntimeConfig, data_dir: PathBuf, state_dir: PathBuf) {
    config.data_dir = data_dir;
    config.state_dir = state_dir.clone();
    config.agent_db = state_dir.join("agents.db");
    config.dag_history_db = state_dir.join("dag_history.db");
    config.bib_db_path = state_dir.join("bib.db");
    config.writing_db_path = state_dir.join("writing.db");
    config.app_db_path = state_dir.join("app.db");
}

fn mock_model_info() -> ModelInfo {
    ModelInfo {
        model_name: "mock-model".into(),
        provider_id: uuid::Uuid::nil(),
        context_length: 8192,
        max_output_tokens: 1024,
        vision_ability: false,
        supports_function_calling: true,
        supports_streaming: true,
        supports_thinking: false,
        thinking_enabled: false,
        max_reasoning_effort: None,
        thinking_required: false,
        thinking_budget: None,
        input_token_price: 0.0,
        output_token_price: 0.0,
    }
}

/// A mock model whose stream scripts one plain-text assistant reply.
fn scripted_text_model(text: &str) -> Model {
    let mut mock = MockApiClient::new();

    let start = Message {
        id: "msg_mock".into(),
        type_: "message".into(),
        role: Role::Assistant,
        content: vec![],
        model: None,
        stop_reason: None,
        stop_sequence: None,
        usage: None,
        request_id: None,
    };
    let events = vec![
        MessageStreamEvent::MessageStart {
            message: start.clone(),
        },
        MessageStreamEvent::ContentBlockStart {
            index: 0,
            content_block: ContentBlock::Text {
                text: String::new(),
            },
        },
        MessageStreamEvent::ContentBlockDelta {
            index: 0,
            delta: ContentBlockDelta::TextDelta {
                text: text.to_string(),
            },
        },
        MessageStreamEvent::ContentBlockStop { index: 0 },
        MessageStreamEvent::MessageDelta {
            delta: MessageDelta {
                stop_reason: Some(StopReason::EndTurn),
                stop_sequence: None,
            },
            usage: MessageDeltaUsage {
                output_tokens: 6,
                input_tokens: Some(11),
                cache_creation_input_tokens: None,
                cache_read_input_tokens: None,
                server_tool_use: None,
            },
        },
        MessageStreamEvent::MessageStop,
    ];
    let final_message = Message {
        id: "msg_mock".into(),
        type_: "message".into(),
        role: Role::Assistant,
        content: vec![ContentBlock::Text {
            text: text.to_string(),
        }],
        model: None,
        stop_reason: Some(StopReason::EndTurn),
        stop_sequence: None,
        usage: None,
        request_id: None,
    };

    mock.expect_request_stream_with_system()
        .times(1)
        .returning(move |_, _, _, _| {
            Ok(MessageStream::from_events(
                events.clone(),
                final_message.clone(),
            ))
        });
    mock.expect_request_stream()
        .returning(|_, _, _| Err(AnthropicError::StreamError("unexpected plain call".into())));

    Model::with_client(mock_model_info(), mock)
}

/// Boot a full gateway daemon (host + driver + axum) on an ephemeral
/// port with an isolated state dir and a scripted mock model.
async fn start_test_gateway(reply: &'static str) -> TestGateway {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .try_init();
    let dir = tempfile::tempdir().unwrap();
    let mut config = RuntimeConfig::default();
    isolate_runtime_paths(
        &mut config,
        dir.path().join("data"),
        dir.path().join("state"),
    );
    // The daemon outlives the turn, so the post-turn memory-consolidation
    // background turn would call the scripted mock a second time and
    // trip mockall. Memory consolidation is off in these wire tests.
    config.use_memory = false;
    config.generate_memory = false;

    let mut host = RuntimeHost::open(&config).await.unwrap();
    let profile_storage = host.infra().profile_storage.clone();
    let _ = profile_storage.seed_defaults_if_empty().await;
    let profiles = profile_storage.list_profiles().await.unwrap_or_default();
    host.set_profiles(profiles.clone());
    host.set_model(Arc::new(ArcSwapOption::from_pointee(Some(
        scripted_text_model(reply),
    ))));

    let hub = EventHub::new();
    let sessions = SessionCache::new();
    let shutdown = CancellationToken::new();

    let state = GatewayState {
        hub: hub.clone(),
        sessions: sessions.clone(),
        control: host.control(),
        infra: host.infra(),
        models: Arc::new(gateway::model_store::ModelStore::open(&config.app_db_path).unwrap()),
        model_slot: Arc::new(ArcSwapOption::default()),
        profiles: Arc::new(profiles),
        started: std::time::Instant::now(),
        shutdown: shutdown.clone(),
        version: "test",
    };

    let bib_shared = state.infra.bib.as_ref().clone();
    let router = router_with_bib(state, TEST_TOKEN.to_string(), bib_shared, None);
    let server = tui_http::server::start(router, "127.0.0.1:0")
        .await
        .unwrap();
    let addr = server.addr();

    tokio::spawn(driver::run(host, hub, sessions, shutdown.clone()));

    TestGateway {
        addr,
        server,
        shutdown,
        _dir: dir,
    }
}

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
    let gateway_daemon = start_test_gateway("The answer is 2.").await;
    let client = gateway_daemon.client();

    // Hydration first.
    let state = client.state().await.unwrap();
    assert!(state.agents.is_empty());
    assert!(!state.profiles.is_empty());
    assert!(state.model_catalog.providers.is_empty() || !state.model_catalog.providers.is_empty());

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
    assert!(refreshed.sessions[&path].len() >= 1);

    // Replay: reconnect with Last-Event-ID below the terminal frame and
    // observe the replayed TurnCompleted again (dedup is the client's
    // job — the raw stream must still deliver it).
    let replay_response = client.connect_events(registered_seq).await.unwrap();
    use eventsource_stream::Eventsource;
    use futures::StreamExt;
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
    let gateway_daemon = start_test_gateway("x").await;

    // Without the token: 401.
    let unauthenticated = GatewayClient::new(&gateway_daemon.addr.to_string(), None).unwrap();
    let err = unauthenticated.state().await.unwrap_err();
    assert_eq!(err.status().unwrap_or(0), 401);

    // With the token: fine.
    gateway_daemon.client().state().await.unwrap();
    gateway_daemon.stop().await;
}
