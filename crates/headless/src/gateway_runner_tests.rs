//! Tests for the gateway-backed runner. The keystone assertion: a run
//! through the daemon emits the **pinned JSONL event contract** (modulo
//! volatile ids and wall-clock), so external consumers survive any
//! change underneath the execution path.

use crate::gateway_runner::{GatewayRunConfig, run_via_gateway_with_client};
use crate::processor::{JsonlProcessor, Outcome, OutputProcessor};
use gateway::testing::start_mock_gateway;

const TEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// Replace values that legitimately differ per run: uuids and
/// wall-clock floats.
fn normalize(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::String(text) => {
            if uuid::Uuid::parse_str(text).is_ok() {
                *text = "<uuid>".to_string();
            }
        }
        serde_json::Value::Object(map) => {
            for (key, entry) in map.iter_mut() {
                if key == "wall_time_secs" {
                    *entry = serde_json::json!(0.0);
                } else {
                    normalize(entry);
                }
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                normalize(item);
            }
        }
        _ => {}
    }
}

fn normalized_lines(jsonl: &str) -> Vec<serde_json::Value> {
    jsonl
        .lines()
        .map(|line| {
            let mut value: serde_json::Value = serde_json::from_str(line).expect("valid JSON line");
            normalize(&mut value);
            value
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gateway_run_streams_the_contract_jsonl() {
    const REPLY: &str = "The answer is 2.";

    let daemon = start_mock_gateway(REPLY).await;
    let client = daemon.client();
    let mut processor = JsonlProcessor::new(Vec::new());
    let summary = tokio::time::timeout(
        TEST_TIMEOUT,
        run_via_gateway_with_client(
            client.clone(),
            GatewayRunConfig {
                prompt: "What is 1+1?".into(),
                ..Default::default()
            },
            &mut processor,
        ),
    )
    .await
    .expect("run completes within timeout")
    .expect("run starts");
    let jsonl = String::from_utf8(processor.into_parts()).unwrap();
    daemon.stop().await;

    // ── Event-stream contract (the pinned JSONL shape) ───────────────
    let events = normalized_lines(&jsonl);
    let expected: Vec<serde_json::Value> = vec![
        serde_json::json!({
            "type": "run.started",
            "run_id": "<uuid>",
            "agent_id": "<uuid>",
            "session_id": "<uuid>",
            "profile": "researcher",
            "model": "mock-model",
        }),
        serde_json::json!({
            "type": "turn.started",
            "turn_id": "<uuid>",
            "session_id": "<uuid>",
        }),
        serde_json::json!({
            "type": "item.completed",
            "item": {
                "id": "msg-1",
                "type": "agent_message",
                "text": REPLY,
            },
        }),
        serde_json::json!({
            "type": "turn.completed",
            "turn_id": "<uuid>",
            "usage": {
                "input_tokens": 11,
                "output_tokens": 6,
                "cache_read_input_tokens": null,
                "cache_creation_input_tokens": null,
            },
        }),
        serde_json::json!({
            "type": "run.ended",
            "run_id": "<uuid>",
            "status": "completed",
            "wall_time_secs": 0.0,
            "usage": {
                "input_tokens": 11,
                "output_tokens": 6,
                "cache_read_input_tokens": null,
                "cache_creation_input_tokens": null,
            },
            "turns": 1,
            "tool_calls": 0,
        }),
    ];
    assert_eq!(
        events, expected,
        "gateway JSONL deviates from the pinned contract\nraw: {jsonl}"
    );

    // ── Summary ──────────────────────────────────────────────────────
    assert_eq!(summary.outcome, Outcome::Completed);
    assert_eq!(summary.agent_path, "/root/headless");
    assert_eq!(summary.last_message.as_deref(), Some(REPLY));
    assert_eq!(summary.turns, 1);
    let usage = summary.usage.expect("usage reported");
    assert_eq!(usage.output_tokens, 6);
    assert_eq!(usage.input_tokens, Some(11));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gateway_run_spawns_the_named_agent_path() {
    let daemon = start_mock_gateway("named reply").await;
    let mut processor = JsonlProcessor::new(Vec::new());
    let summary = tokio::time::timeout(
        TEST_TIMEOUT,
        run_via_gateway_with_client(
            daemon.client(),
            GatewayRunConfig {
                prompt: "ping".into(),
                agent_name: Some("research_run".into()),
                ..Default::default()
            },
            &mut processor,
        ),
    )
    .await
    .expect("run completes within timeout")
    .expect("run starts");
    daemon.stop().await;

    assert_eq!(summary.agent_path, "/root/research_run");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gateway_run_cleans_up_the_one_shot_agent() {
    let daemon = start_mock_gateway("done").await;
    let client = daemon.client();
    let mut processor = JsonlProcessor::new(Vec::new());
    let summary = tokio::time::timeout(
        TEST_TIMEOUT,
        run_via_gateway_with_client(
            client.clone(),
            GatewayRunConfig {
                prompt: "ping".into(),
                ..Default::default()
            },
            &mut processor,
        ),
    )
    .await
    .expect("completes within timeout")
    .expect("starts");
    assert_eq!(summary.outcome, Outcome::Completed);

    // The one-shot agent is shut down after the run — /state no longer
    // lists it (its stored record remains for identity restore).
    let gone = tokio::time::timeout(TEST_TIMEOUT, async {
        loop {
            let state = client.state().await.unwrap();
            if !state.agents.iter().any(|a| a.path == summary.agent_path) {
                return true;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("agent removed from live registry");
    assert!(gone);
    daemon.stop().await;
}
