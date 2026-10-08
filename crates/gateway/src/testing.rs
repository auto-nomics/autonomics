//! Test harness for gateway consumers: boots a full daemon (host +
//! driver + axum on an ephemeral port) with an isolated state dir and a
//! scripted mock model — no network beyond loopback, no API keys.
//!
//! Feature-gated (`test-util`): it drags `MockApiClient` in via
//! agentik-sdk's test-util feature and must never ship in release builds.

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

use crate::client::GatewayClient;
use crate::driver::{self, SessionCache};
use crate::hub::EventHub;
use crate::server::DockerHubClient;
use crate::server::{GatewayState, router_with_bib};
use runtime::{RuntimeConfig, RuntimeHost};
use tokio_util::sync::CancellationToken;

/// Default wall-clock guard for tests driving this harness.
pub const TEST_TIMEOUT: Duration = Duration::from_secs(30);

/// A booted test daemon. Dropping the [`TestGateway`] (or calling
/// [`TestGateway::stop`]) cancels the driver and shuts the server down;
/// the temp state dir is removed on drop.
pub struct TestGateway {
    pub addr: std::net::SocketAddr,
    pub token: String,
    server: api_server::HttpServerHandle,
    shutdown: CancellationToken,
    _dir: tempfile::TempDir,
    /// The daemon's shared infrastructure — kept so tests can reach
    /// deterministic handles (e.g. the skills manager) without racing
    /// the process-global singleton other parallel setups swap.
    pub infra: runtime::SharedInfra,
}

impl TestGateway {
    /// A connected client (bearer-authenticated).
    pub fn client(&self) -> GatewayClient {
        GatewayClient::new(&self.addr.to_string(), Some(&self.token)).unwrap()
    }

    /// Graceful stop (driver cancel + HTTP shutdown).
    pub async fn stop(self) {
        self.shutdown.cancel();
        let _ = self.server.shutdown().await;
    }
}

/// Redirect EVERY persistent path under the test temp dir —
/// `RuntimeConfig::default()` bakes absolute `~/.autonomics` paths for
/// the databases at resolution time, and an ephemeral run that still
/// writes to the real installation is not ephemeral.
pub fn isolate_runtime_paths(config: &mut RuntimeConfig, data_dir: PathBuf, state_dir: PathBuf) {
    config.data_dir = data_dir;
    config.state_dir = state_dir.clone();
    config.agent_db = state_dir.join("agents.db");
    config.dag_history_db = state_dir.join("dag_history.db");
    config.bib_db_path = state_dir.join("bib.db");
    config.writing_db_path = state_dir.join("writing.db");
    config.app_db_path = state_dir.join("app.db");
}

pub fn mock_model_info() -> ModelInfo {
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

/// A mock model whose stream scripts one plain-text assistant reply
/// ending with `EndTurn` (usage: 11 input / 6 output tokens — asserted by
/// consumers' JSONL contract tests).
pub fn scripted_text_model(text: &str) -> Model {
    scripted_text_model_n(text, 1)
}

/// Like [`scripted_text_model`], but admits `times` scripted calls — a
/// harness that reuses one daemon across runs (session resume) needs
/// more than one.
pub fn scripted_text_model_n(text: &str, times: usize) -> Model {
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
        .times(times)
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

/// A mock model that fails immediately with a non-retryable error —
/// surfaces as a failed turn, never a retry loop.
pub fn auth_failure_model() -> Model {
    let mut mock = MockApiClient::new();
    mock.expect_request_stream_with_system()
        .returning(|_, _, _, _| {
            Err(AnthropicError::Authentication {
                message: "bad key".into(),
                status: 401,
            })
        });
    Model::with_client(mock_model_info(), mock)
}

/// A mock model whose requests always fail with a retryable error — the
/// agent backs off (first sleep: 1s), giving a short run timeout a window
/// to fire mid-turn.
pub fn always_retrying_model() -> Model {
    let mut mock = MockApiClient::new();
    mock.expect_request_stream_with_system()
        .returning(|_, _, _, _| Err(AnthropicError::StreamError("transient".into())));
    Model::with_client(mock_model_info(), mock)
}

/// Boot a full gateway daemon (host + driver + axum) on an ephemeral
/// port with an isolated state dir and a scripted mock model.
///
/// Memory consolidation is disabled: the daemon outlives the scripted
/// turn, and the post-turn memory-consolidation background turn would
/// call the mock a second time and trip mockall.
pub async fn start_mock_gateway(reply: &'static str) -> TestGateway {
    start_mock_gateway_with_model(scripted_text_model(reply)).await
}

/// [`start_mock_gateway`] with an explicit mock model — failure and
/// retry harnesses inject their own scripting.
pub async fn start_mock_gateway_with_model(model: Model) -> TestGateway {
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
    config.use_memory = false;
    config.generate_memory = false;

    let mut host = RuntimeHost::open(&config).await.unwrap();
    // One slot shared by the host and the gateway state — exactly like
    // the real daemon wires it.
    let model_slot: Arc<ArcSwapOption<Model>> = Arc::new(ArcSwapOption::from_pointee(Some(model)));
    host.set_model(model_slot.clone());

    let hub = EventHub::new();
    let sessions = SessionCache::new();
    let shutdown = CancellationToken::new();

    let models = Arc::new(crate::model_store::ModelStore::open(&config.app_db_path).unwrap());
    // Report the injected mock through the same channel a real daemon
    // uses (`active_model` in the settings table), so `/state` consumers
    // (e.g. the headless runner's run.started.model) see it.
    let _ = models.put_setting("active_model", "mock:mock-model");
    // Same install-later pattern as the daemon: the bound address exists
    // only after `start()` (tests bind an ephemeral port).
    let addr_slot: Arc<ArcSwapOption<String>> = Arc::new(ArcSwapOption::default());
    let state = GatewayState {
        hub: hub.clone(),
        sessions: sessions.clone(),
        control: host.control(),
        infra: host.infra(),
        dockerhub: Arc::new(DockerHubClient::default()),
        models,
        model_slot,
        addr: addr_slot.clone(),
        started: std::time::Instant::now(),
        shutdown: shutdown.clone(),
        version: "test",
    };

    let bib_shared = state.infra.bib.as_ref().clone();
    let token = uuid::Uuid::new_v4().simple().to_string();
    let router = router_with_bib(state, token.clone(), bib_shared, None);
    let server = api_server::server::start(router, "127.0.0.1:0")
        .await
        .unwrap();
    addr_slot.store(Some(Arc::new(server.addr().to_string())));
    let addr = server.addr();

    let infra = host.infra();
    tokio::spawn(driver::run(host, hub, sessions, shutdown.clone()));

    TestGateway {
        addr,
        token,
        server,
        shutdown,
        _dir: dir,
        infra,
    }
}
