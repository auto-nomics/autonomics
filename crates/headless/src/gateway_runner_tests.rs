//! Tests for the gateway-backed runner. The keystone assertion: a run
//! through the daemon produces the **same JSONL event stream** as the
//! in-process `run_task` (modulo volatile ids and wall-clock), pinning
//! the external contract while the execution path changes underneath.

use crate::gateway_runner::{GatewayRunConfig, run_via_gateway_with_client};
use crate::processor::{JsonlProcessor, Outcome, OutputProcessor};
use crate::{RunTaskConfig, run_task};
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
async fn gateway_run_streams_the_same_events_as_in_process() {
    const REPLY: &str = "The answer is 2.";

    // ── In-process reference run (ephemeral, isolated temp state) ────
    let (mut config, _guard) = RunTaskConfig::ephemeral("What is 1+1?");
    // Match the mock daemon harness: memory consolidation off (the
    // daemon outlives the scripted turn and the mock allows one call).
    config.runtime_config.use_memory = false;
    config.runtime_config.generate_memory = false;
    config.model = Some(gateway::testing::scripted_text_model(REPLY));
    config.model_name = Some("mock-model".into());
    let mut in_process = JsonlProcessor::new(Vec::new());
    let in_process_summary = tokio::time::timeout(TEST_TIMEOUT, run_task(config, &mut in_process))
        .await
        .expect("in-process run completes within timeout")
        .expect("in-process run starts");
    let in_process_jsonl = String::from_utf8(in_process.into_parts()).unwrap();

    // ── Gateway run (mock daemon, isolated temp state) ───────────────
    let daemon = start_mock_gateway(REPLY).await;
    let client = daemon.client();
    let mut gateway = JsonlProcessor::new(Vec::new());
    let gateway_summary = tokio::time::timeout(
        TEST_TIMEOUT,
        run_via_gateway_with_client(
            client,
            GatewayRunConfig {
                prompt: "What is 1+1?".into(),
                ..Default::default()
            },
            &mut gateway,
        ),
    )
    .await
    .expect("gateway run completes within timeout")
    .expect("gateway run starts");
    let gateway_jsonl = String::from_utf8(gateway.into_parts()).unwrap();
    daemon.stop().await;

    // ── Event-stream parity (the contract) ───────────────────────────
    let left = normalized_lines(&in_process_jsonl);
    let right = normalized_lines(&gateway_jsonl);
    assert_eq!(
        left, right,
        "gateway run must stream the same RunEvent JSONL as in-process\n\
         in-process: {in_process_jsonl}\n  gateway: {gateway_jsonl}"
    );

    // ── Summary parity ───────────────────────────────────────────────
    assert_eq!(gateway_summary.outcome, Outcome::Completed);
    assert_eq!(in_process_summary.outcome, Outcome::Completed);
    assert_eq!(gateway_summary.agent_path, "/root/headless");
    assert_eq!(gateway_summary.last_message.as_deref(), Some(REPLY));
    assert_eq!(gateway_summary.turns, 1);
    let usage = gateway_summary.usage.expect("usage reported");
    assert_eq!(usage.output_tokens, 6);
    assert_eq!(usage.input_tokens, Some(11));
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
