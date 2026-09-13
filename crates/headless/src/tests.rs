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
use agentik_types::streaming::{ContentBlockDelta, MessageDelta, MessageDeltaUsage, MessageStreamEvent};
use std::time::Duration;

/// Wall-clock guard: a hung run fails the test instead of the suite.
const TEST_TIMEOUT: Duration = Duration::from_secs(30);

fn test_config(dir: &tempfile::TempDir) -> RunTaskConfig {
    let mut runtime_config = RuntimeConfig::default();
    runtime_config.data_dir = dir.path().join("data");
    runtime_config.state_dir = dir.path().join("state");
    RunTaskConfig::new("What is 1+1?", runtime_config)
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
            Ok(MessageStream::from_events(events.clone(), final_message.clone()))
        });
    mock.expect_request_stream()
        .returning(|_, _, _| Err(AnthropicError::StreamError("unexpected plain call".into())));

    Model::with_client(mock_model_info(), mock)
}

/// A mock model that fails immediately with a non-retryable error.
fn auth_failure_model() -> Model {
    let mut mock = MockApiClient::new();
    mock.expect_request_stream_with_system().returning(|_, _, _, _| {
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
    config.model = Some(scripted_text_model("The answer is 2.", 6, 11));
    config.model_name = Some("mock-model".into());

    let mut processor = JsonlProcessor::new(Vec::new());
    let summary = tokio::time::timeout(TEST_TIMEOUT, run_task(config, &mut processor))
        .await
        .expect("run completes within timeout")
        .expect("startup succeeds");

    assert_eq!(summary.outcome, Outcome::Completed);
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
    assert!(progress.contains("run started"), "progress shows start: {progress}");
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
