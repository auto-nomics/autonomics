//! End-to-end tests for `run_task`, driven entirely by a scripted
//! `MockApiClient` — no network, no API key. This is the reliability
//! keystone of the headless route: the full spawn → prompt → translate →
//! shutdown lifecycle runs in CI.

use super::*;
use crate::processor::{HumanProcessor, JsonlProcessor, Outcome, OutputProcessor};
use agentik_sdk::model::ModelInfo;
use agentik_sdk::provider::client::MockApiClient;
use agentik_sdk::streaming::MessageStream;
use agentik_types::errors::AnthropicError;
use agentik_types::messages::{ContentBlock, Message, Role, StopReason};
use agentik_types::streaming::{
    ContentBlockDelta, MessageDelta, MessageDeltaUsage, MessageStreamEvent,
};
use std::path::PathBuf;
use std::time::Duration;

/// Wall-clock guard: a hung run fails the test instead of the suite.
const TEST_TIMEOUT: Duration = Duration::from_secs(30);

fn test_config(dir: &tempfile::TempDir) -> RunTaskConfig {
    let mut runtime_config = RuntimeConfig::default();
    isolate_runtime_paths(
        &mut runtime_config,
        dir.path().join("data"),
        dir.path().join("state"),
    );
    RunTaskConfig::new("What is 1+1?", runtime_config)
}

/// Redirect EVERY persistent path (databases included) under the test
/// temp dir. `RuntimeConfig::default()` bakes absolute paths
/// (`~/.autonomics/...`) for agent/bib/writing/app DBs at resolution
/// time, so overriding only `state_dir` after the fact silently leaks
/// test agents and sessions into the real installation.
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
        provider_id: Uuid::nil(),
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

/// Build a mock model whose stream scripts one plain-text assistant
/// reply ending with `EndTurn`, carrying the given usage numbers.
fn scripted_text_model(text: &str, output_tokens: u64, input_tokens: u64) -> Model {
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
                output_tokens,
                input_tokens: Some(input_tokens),
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

    // The session calls the `with_system` variant when a system prompt is
    // set (it always is — profiles carry an agent identity); mock the
    // plain variant too so a promptless path stays covered.
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

/// A mock model that fails immediately with a non-retryable error.
fn auth_failure_model() -> Model {
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

fn tags_of(jsonl: &str) -> Vec<String> {
    jsonl
        .lines()
        .map(|line| {
            let value: serde_json::Value = serde_json::from_str(line).expect("valid JSON line");
            value["type"].as_str().expect("tag present").to_string()
        })
        .collect()
}

#[tokio::test]
async fn scripted_turn_completes_and_streams_jsonl() {
    let dir = tempfile::tempdir().unwrap();
    let mut config = test_config(&dir);
    let run_id = config.run_id;
    config.model = Some(scripted_text_model("The answer is 2.", 6, 11));
    config.model_name = Some("mock-model".into());

    let mut processor = JsonlProcessor::new(Vec::new());
    let summary = tokio::time::timeout(TEST_TIMEOUT, run_task(config, &mut processor))
        .await
        .expect("run completes within timeout")
        .expect("startup succeeds");

    assert_eq!(summary.outcome, Outcome::Completed);
    assert_eq!(summary.run_id, run_id);
    assert_eq!(summary.last_message.as_deref(), Some("The answer is 2."));
    assert_eq!(summary.turns, 1);
    assert_eq!(summary.agent_path, "/root/headless");
    let usage = summary.usage.expect("usage reported");
    assert_eq!(usage.output_tokens, 6);
    assert_eq!(usage.input_tokens, Some(11));

    let jsonl = String::from_utf8(processor.into_parts()).unwrap();
    let tags = tags_of(&jsonl);
    assert_eq!(tags.first().map(String::as_str), Some("run.started"));
    assert_eq!(tags.last().map(String::as_str), Some("run.ended"));
    assert_eq!(tags.iter().filter(|t| *t == "turn.started").count(), 1);
    assert_eq!(tags.iter().filter(|t| *t == "turn.completed").count(), 1);
    assert_eq!(tags.iter().filter(|t| *t == "item.completed").count(), 1);
    assert!(!tags.contains(&"turn.failed".to_string()));

    // run.ended carries the terminal status and usage.
    let ended: serde_json::Value = serde_json::from_str(jsonl.lines().last().unwrap()).unwrap();
    assert_eq!(ended["status"], "completed");
    assert_eq!(ended["run_id"], run_id.to_string());
    assert_eq!(ended["usage"]["output_tokens"], 6);
}

#[tokio::test]
async fn human_mode_stdout_receives_only_final_message() {
    let dir = tempfile::tempdir().unwrap();
    let mut config = test_config(&dir);
    config.model = Some(scripted_text_model("The answer is 2.", 6, 11));
    config.model_name = Some("mock-model".into());

    let mut processor = HumanProcessor::new(Vec::new(), Vec::new());
    let summary = tokio::time::timeout(TEST_TIMEOUT, run_task(config, &mut processor))
        .await
        .expect("run completes within timeout")
        .expect("startup succeeds");
    assert_eq!(summary.outcome, Outcome::Completed);

    let (progress, output) = processor.into_parts();
    let output = String::from_utf8(output).unwrap();
    assert_eq!(output, "The answer is 2.\n");
    let progress = String::from_utf8(progress).unwrap();
    assert!(
        progress.contains("run started"),
        "progress shows start: {progress}"
    );
    assert!(
        !progress.contains("The answer is 2."),
        "final message never leaks to progress: {progress}"
    );
}

#[tokio::test]
async fn non_retryable_failure_reports_failed_turn() {
    let dir = tempfile::tempdir().unwrap();
    let mut config = test_config(&dir);
    config.model = Some(auth_failure_model());
    config.model_name = Some("mock-model".into());

    let mut processor = JsonlProcessor::new(Vec::new());
    let summary = tokio::time::timeout(TEST_TIMEOUT, run_task(config, &mut processor))
        .await
        .expect("run completes within timeout")
        .expect("startup succeeds");

    assert_eq!(summary.outcome, Outcome::Failed);
    assert_eq!(summary.last_message, None);

    let jsonl = String::from_utf8(processor.into_parts()).unwrap();
    let tags = tags_of(&jsonl);
    assert!(tags.contains(&"turn.failed".to_string()));
    let ended: serde_json::Value = serde_json::from_str(jsonl.lines().last().unwrap()).unwrap();
    assert_eq!(ended["status"], "failed");
    // The failure detail reaches consumers.
    assert!(
        jsonl.contains("bad key"),
        "failure message surfaced: {jsonl}"
    );
}

#[tokio::test]
async fn unknown_profile_is_a_startup_error() {
    let dir = tempfile::tempdir().unwrap();
    let mut config = test_config(&dir);
    config.profile = Some("no-such-profile".into());
    config.model = Some(auth_failure_model());

    let mut processor = JsonlProcessor::new(Vec::new());
    let result = tokio::time::timeout(TEST_TIMEOUT, run_task(config, &mut processor))
        .await
        .expect("resolves within timeout");
    assert!(matches!(result, Err(RunError::NoProfile { .. })));
}

#[tokio::test]
async fn unknown_session_is_a_startup_error() {
    let dir = tempfile::TempDir::new().unwrap();
    let mut config = test_config(&dir);
    config.model = Some(auth_failure_model());
    config.session = Some(Uuid::new_v4());

    let mut processor = JsonlProcessor::new(Vec::new());
    let result = tokio::time::timeout(TEST_TIMEOUT, run_task(config, &mut processor))
        .await
        .expect("resolves within timeout");
    assert!(matches!(result, Err(RunError::SessionSwitch { .. })));
}

/// A mock model whose requests always fail with a retryable error — the
/// agent backs off (first sleep: 1s), giving a short run timeout a window
/// to fire mid-turn.
fn always_retrying_model() -> Model {
    let mut mock = MockApiClient::new();
    mock.expect_request_stream_with_system()
        .returning(|_, _, _, _| Err(AnthropicError::StreamError("transient".into())));
    Model::with_client(mock_model_info(), mock)
}

#[tokio::test]
async fn timeout_cancels_mid_turn() {
    let dir = tempfile::tempdir().unwrap();
    let mut config = test_config(&dir);
    config.model = Some(always_retrying_model());
    config.model_name = Some("mock-model".into());
    config.timeout = Some(Duration::from_millis(300));

    let mut processor = JsonlProcessor::new(Vec::new());
    let summary = tokio::time::timeout(TEST_TIMEOUT, run_task(config, &mut processor))
        .await
        .expect("run completes within timeout")
        .expect("startup succeeds");

    assert_eq!(summary.outcome, Outcome::Cancelled);
    let jsonl = String::from_utf8(processor.into_parts()).unwrap();
    assert!(
        jsonl.contains("run timed out after"),
        "timeout surfaced: {jsonl}"
    );
    let ended: serde_json::Value = serde_json::from_str(jsonl.lines().last().unwrap()).unwrap();
    assert_eq!(ended["status"], "cancelled");
}

#[tokio::test]
async fn frontend_cancel_cancels_mid_turn() {
    let dir = tempfile::TempDir::new().unwrap();
    let mut config = test_config(&dir);
    config.model = Some(always_retrying_model());
    config.model_name = Some("mock-model".into());

    let cancel = config.cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(100)).await;
        cancel.cancel();
    });

    let mut processor = JsonlProcessor::new(Vec::new());
    let summary = tokio::time::timeout(TEST_TIMEOUT, run_task(config, &mut processor))
        .await
        .expect("run completes within timeout")
        .expect("startup succeeds");

    assert_eq!(summary.outcome, Outcome::Cancelled);
    let jsonl = String::from_utf8(processor.into_parts()).unwrap();
    assert!(jsonl.contains("run cancelled"), "events: {jsonl}");
}

#[test]
fn translation_pairs_tools_and_counts_them() {
    let mut state = TranslationState::default();
    let input = vec![
        AgentEvent::TurnStarted {
            turn_id: Uuid::nil(),
            session_id: Uuid::nil(),
            delegation_id: None,
        },
        AgentEvent::ToolCall {
            name: "run_bash".into(),
            input: Value::Null,
        },
        AgentEvent::ToolCall {
            name: "lit_search".into(),
            input: Value::Null,
        },
        AgentEvent::ToolResult {
            ok: true,
            content: "first".into(),
        },
        AgentEvent::ToolResult {
            ok: false,
            content: "second".into(),
        },
        AgentEvent::TurnCompleted {
            turn_id: Uuid::nil(),
            session_id: Uuid::nil(),
            delegation_id: None,
            status: TurnExecutionStatus::Completed,
        },
    ];
    let mut out = vec![];
    for event in input {
        out.extend(state.translate("agent", event));
    }
    assert_eq!(state.tool_calls, 2);
    assert_eq!(
        out.iter()
            .filter(|e| matches!(e, RunEvent::ItemStarted(_)))
            .count(),
        2
    );
    // FIFO pairing: the first completed tool is run_bash with ok=true.
    let completed: Vec<&RunEvent> = out
        .iter()
        .filter(|e| matches!(e, RunEvent::ItemCompleted(_)))
        .collect();
    let RunEvent::ItemCompleted(ItemEvent {
        item:
            RunItem {
                details: RunItemDetails::ToolCall(tool),
                ..
            },
        ..
    }) = completed[0]
    else {
        panic!("expected tool item");
    };
    assert_eq!(tool.tool, "run_bash");
    assert_eq!(tool.result.as_deref(), Some("first"));
    assert_eq!(tool.ok, Some(true));
}

#[tokio::test]
async fn session_resume_continues_the_same_session() {
    let dir = tempfile::tempdir().unwrap();

    // Run 1: capture the session id from turn.started.
    let mut config = test_config(&dir);
    config.model = Some(scripted_text_model("first answer", 3, 5));
    config.model_name = Some("mock-model".into());
    let mut processor = JsonlProcessor::new(Vec::new());
    let first = tokio::time::timeout(TEST_TIMEOUT, run_task(config, &mut processor))
        .await
        .expect("first run completes")
        .expect("startup succeeds");
    assert_eq!(first.outcome, Outcome::Completed);
    let jsonl = String::from_utf8(processor.into_parts()).unwrap();
    let session_id: Uuid = jsonl
        .lines()
        .map(|l| serde_json::from_str::<serde_json::Value>(l).unwrap())
        .find(|v| v["type"] == "turn.started")
        .and_then(|v| v["session_id"].as_str().map(String::from))
        .and_then(|s| s.parse().ok())
        .expect("turn.started carries a session id");

    // Run 2: same state dir (agent restored by path), resumed session.
    let mut config = test_config(&dir);
    config.model = Some(scripted_text_model("second answer", 4, 7));
    config.model_name = Some("mock-model".into());
    config.session = Some(session_id);
    let mut processor = JsonlProcessor::new(Vec::new());
    let second = tokio::time::timeout(TEST_TIMEOUT, run_task(config, &mut processor))
        .await
        .expect("second run completes")
        .expect("startup succeeds");
    assert_eq!(second.outcome, Outcome::Completed);
    assert_eq!(second.profile, first.profile, "same stored profile picked");

    let jsonl = String::from_utf8(processor.into_parts()).unwrap();
    let resumed_id: Uuid = jsonl
        .lines()
        .map(|l| serde_json::from_str::<serde_json::Value>(l).unwrap())
        .find(|v| v["type"] == "turn.started")
        .and_then(|v| v["session_id"].as_str().map(String::from))
        .and_then(|s| s.parse().ok())
        .expect("turn.started carries a session id");
    assert_eq!(
        resumed_id, session_id,
        "run 2 resumed run 1's session; events:\n{jsonl}"
    );
}

#[tokio::test]
async fn ephemeral_run_uses_throwaway_state() {
    let (mut config, _guard) = RunTaskConfig::ephemeral("ephemeral prompt");
    config.model = Some(scripted_text_model("ephemeral answer", 2, 3));
    config.model_name = Some("mock-model".into());

    let state_root = config.runtime_config.state_dir.clone();
    let mut processor = JsonlProcessor::new(Vec::new());
    let summary = tokio::time::timeout(TEST_TIMEOUT, run_task(config, &mut processor))
        .await
        .expect("run completes")
        .expect("startup succeeds");
    assert_eq!(summary.outcome, Outcome::Completed);
    assert!(
        state_root.starts_with(std::env::temp_dir()),
        "state lives under the temp root: {}",
        state_root.display()
    );
    assert!(state_root.exists(), "state dir was used during the run");
}
