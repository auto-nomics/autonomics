//! End-to-end tests for the gateway-backed headless run, driven entirely
//! by scripted mock models — no network beyond loopback, no API keys.
//! This is the reliability keystone of the headless route: the full
//! spawn → prompt → translate → shutdown lifecycle runs in CI.

use std::time::Duration;

use super::*;
use crate::gateway_runner::{GatewayRunConfig, run_via_gateway_with_client};
use crate::processor::{HumanProcessor, JsonlProcessor, Outcome, OutputProcessor};
use gateway::testing::{
    always_retrying_model, auth_failure_model, scripted_text_model_n, start_mock_gateway,
    start_mock_gateway_with_model,
};
use tokio_util::sync::CancellationToken;

/// Wall-clock guard: a hung run fails the test instead of the suite.
const TEST_TIMEOUT: Duration = Duration::from_secs(30);

/// One gateway run with the JSONL processor; returns the summary and the
/// raw JSONL text (the contract surface most assertions read).
async fn run_jsonl(
    client: gateway::GatewayClient,
    config: GatewayRunConfig,
) -> Result<(RunSummary, String), RunError> {
    let mut processor = JsonlProcessor::new(Vec::new());
    let summary = tokio::time::timeout(
        TEST_TIMEOUT,
        run_via_gateway_with_client(client, config, &mut processor),
    )
    .await
    .expect("run completes within timeout")?;
    let jsonl = String::from_utf8(processor.into_parts()).unwrap();
    Ok((summary, jsonl))
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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn scripted_turn_completes_and_streams_jsonl() {
    let daemon = start_mock_gateway("The answer is 2.").await;
    let run_id = Uuid::new_v4();
    let (summary, jsonl) = run_jsonl(
        daemon.client(),
        GatewayRunConfig {
            run_id,
            prompt: "What is 1+1?".into(),
            ..Default::default()
        },
    )
    .await
    .expect("startup succeeds");
    daemon.stop().await;

    assert_eq!(summary.outcome, Outcome::Completed);
    assert_eq!(summary.run_id, run_id);
    assert_eq!(summary.last_message.as_deref(), Some("The answer is 2."));
    assert_eq!(summary.turns, 1);
    assert_eq!(summary.agent_path, "/root/headless");
    let usage = summary.usage.expect("usage reported");
    assert_eq!(usage.output_tokens, 6);
    assert_eq!(usage.input_tokens, Some(11));

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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn human_mode_stdout_receives_only_final_message() {
    let daemon = start_mock_gateway("The answer is 2.").await;
    let mut processor = HumanProcessor::new(Vec::new(), Vec::new());
    let summary = tokio::time::timeout(
        TEST_TIMEOUT,
        run_via_gateway_with_client(
            daemon.client(),
            GatewayRunConfig {
                prompt: "What is 1+1?".into(),
                ..Default::default()
            },
            &mut processor,
        ),
    )
    .await
    .expect("run completes within timeout")
    .expect("startup succeeds");
    daemon.stop().await;
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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn non_retryable_failure_reports_failed_turn() {
    let daemon = start_mock_gateway_with_model(auth_failure_model()).await;
    let (summary, jsonl) = run_jsonl(
        daemon.client(),
        GatewayRunConfig {
            prompt: "What is 1+1?".into(),
            ..Default::default()
        },
    )
    .await
    .expect("startup succeeds");
    daemon.stop().await;

    assert_eq!(summary.outcome, Outcome::Failed);
    assert_eq!(summary.last_message, None);

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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn unknown_profile_is_a_startup_error() {
    let daemon = start_mock_gateway("unused").await;
    let result = run_jsonl(
        daemon.client(),
        GatewayRunConfig {
            prompt: "What is 1+1?".into(),
            profile: Some("no-such-profile".into()),
            ..Default::default()
        },
    )
    .await;
    daemon.stop().await;
    assert!(matches!(result, Err(RunError::NoProfile { .. })));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn unknown_session_is_a_startup_error() {
    let daemon = start_mock_gateway("unused").await;
    let result = run_jsonl(
        daemon.client(),
        GatewayRunConfig {
            prompt: "What is 1+1?".into(),
            session: Some(Uuid::new_v4()),
            ..Default::default()
        },
    )
    .await;
    daemon.stop().await;
    assert!(matches!(result, Err(RunError::SessionSwitch { .. })));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn timeout_cancels_mid_turn() {
    let daemon = start_mock_gateway_with_model(always_retrying_model()).await;
    let (summary, jsonl) = run_jsonl(
        daemon.client(),
        GatewayRunConfig {
            prompt: "What is 1+1?".into(),
            timeout: Some(Duration::from_millis(300)),
            ..Default::default()
        },
    )
    .await
    .expect("startup succeeds");
    daemon.stop().await;

    assert_eq!(summary.outcome, Outcome::Cancelled);
    let jsonl_str = jsonl.as_str();
    assert!(
        jsonl_str.contains("run timed out after"),
        "timeout surfaced: {jsonl}"
    );
    let ended: serde_json::Value = serde_json::from_str(jsonl.lines().last().unwrap()).unwrap();
    assert_eq!(ended["status"], "cancelled");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn frontend_cancel_cancels_mid_turn() {
    let daemon = start_mock_gateway_with_model(always_retrying_model()).await;
    let cancel = CancellationToken::new();
    tokio::spawn({
        let cancel = cancel.clone();
        async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
            cancel.cancel();
        }
    });

    let (summary, jsonl) = run_jsonl(
        daemon.client(),
        GatewayRunConfig {
            prompt: "What is 1+1?".into(),
            cancel,
            ..Default::default()
        },
    )
    .await
    .expect("startup succeeds");
    daemon.stop().await;

    assert_eq!(summary.outcome, Outcome::Cancelled);
    let jsonl_str = jsonl.as_str();
    assert!(jsonl_str.contains("run cancelled"), "events: {jsonl}");
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
            telemetry: agentik_types::TurnTelemetry {
                total_tool_use: 2,
                ..Default::default()
            },
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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn session_resume_continues_the_same_session() {
    let daemon = start_mock_gateway_with_model(scripted_text_model_n("answer", 2)).await;
    let client = daemon.client();

    // Run 1: capture the session id from turn.started.
    let (first, first_jsonl) = run_jsonl(
        client.clone(),
        GatewayRunConfig {
            prompt: "first prompt".into(),
            agent_name: Some("named_resume".into()),
            ..Default::default()
        },
    )
    .await
    .expect("startup succeeds");
    assert_eq!(first.outcome, Outcome::Completed);
    let session_id: Uuid = first_jsonl
        .lines()
        .map(|l| serde_json::from_str::<serde_json::Value>(l).unwrap())
        .find(|v| v["type"] == "turn.started")
        .and_then(|v| v["session_id"].as_str().map(String::from))
        .and_then(|s| s.parse().ok())
        .expect("turn.started carries a session id");

    // Wait for the post-shutdown snapshot flush to reach storage before
    // resuming (run 2 restores the agent from that storage) — polling
    // turns a snapshot-flush race into a deterministic wait.
    tokio::time::timeout(TEST_TIMEOUT, async {
        loop {
            let agents = client.list_storage_agents().await.unwrap();
            if let Some(record) = agents.iter().find(|r| r.name == "/root/named_resume") {
                let sessions = client.list_stored_sessions(record.id).await.unwrap();
                if sessions.iter().any(|s| s.id == session_id) {
                    return;
                }
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("session snapshot flushed to storage");

    // Run 2: same daemon (agent restored by path), resumed session.
    let (second, second_jsonl) = run_jsonl(
        client.clone(),
        GatewayRunConfig {
            prompt: "second prompt".into(),
            agent_name: Some("named_resume".into()),
            session: Some(session_id),
            ..Default::default()
        },
    )
    .await
    .expect("startup succeeds");
    daemon.stop().await;
    assert_eq!(second.outcome, Outcome::Completed);
    assert_eq!(second.profile, first.profile, "same stored profile picked");

    let resumed_id: Uuid = second_jsonl
        .lines()
        .map(|l| serde_json::from_str::<serde_json::Value>(l).unwrap())
        .find(|v| v["type"] == "turn.started")
        .and_then(|v| v["session_id"].as_str().map(String::from))
        .and_then(|s| s.parse().ok())
        .expect("turn.started carries a session id");
    assert_eq!(
        resumed_id, session_id,
        "run 2 resumed run 1's session; events:\n{second_jsonl}"
    );
}
