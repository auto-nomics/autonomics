//! Struct-level tests that require access to the private tool structs
//! (not accessible from `host.rs`), plus policy guards over the tool
//! registry and the visibility filtering of the read tools.

use super::get_agent_info::GetAgentInfoInput;
use super::list_agents::ListAgentsInput;
use super::*;
use crate::control::HostCommand;
use crate::host::HostEvent;
use agentik_core::tools::{ExecutionMode, ToolFunction, ToolRegistration};

fn test_control() -> (
    HostControl,
    tokio::sync::mpsc::UnboundedReceiver<HostCommand>,
) {
    let (cmd_tx, cmd_rx) = tokio::sync::mpsc::unbounded_channel::<HostCommand>();
    let (event_tx, _) = tokio::sync::broadcast::channel::<HostEvent>(1);
    (HostControl::new(cmd_tx, event_tx), cmd_rx)
}

/// `DelegateToTool` is Async with a 24h cap — the result is pulled on
/// demand after the target child finishes its turn.
#[test]
fn delegate_tool_is_async_with_day_timeout() {
    let (control, _rx) = test_control();
    let tool = DelegateToTool {
        control,
        caller_path: "/root/caller".into(),
    };

    assert_eq!(tool.execution_mode(), ExecutionMode::Async);
    assert_eq!(tool.timeout_seconds(), 86400);
}

/// The tool registry must expose exactly the delegate-only, one-hop
/// visibility tool set. Guards against accidental re-exposure of the
/// removed communication channels (send_message, set_termination,
/// get_network_status, inject_prompts).
#[test]
fn host_tool_registry_matches_policy() {
    let (control, _rx) = test_control();
    let path = agentik_types::AgentPath::try_from("/root/caller").unwrap();
    let registrations: Vec<ToolRegistration> =
        host_tools(Some(control), &path, agentik_core::AgentKind::Researcher);
    let mut names: Vec<String> = registrations
        .iter()
        .map(|r| r.definition.name.clone())
        .collect();
    names.sort();

    assert_eq!(
        names,
        vec![
            "delegate_to",
            "get_agent_history",
            "get_agent_info",
            "interrupt_agent",
            "list_agents",
            "list_delegations",
            "spawn_agent",
        ]
    );
}

fn agent_info(path: &str) -> crate::control::AgentInfo {
    crate::control::AgentInfo {
        name: path.rsplit('/').next().unwrap_or(path).to_string(),
        path: path.to_string(),
        agent_id: None,
        summary: String::new(),
        tags: vec![],
        expertise: vec![],
        tools: vec![],
        status: agentik_types::AgentLifecycleStatus::Idle,
        last_event: None,
        model: None,
        context_length: None,
    }
}

fn synthetic_status() -> crate::control::HostStatus {
    crate::control::HostStatus {
        agents: vec![
            agent_info("/root/p"),
            agent_info("/root/p/c1"),
            agent_info("/root/p/c1/gc"),
            agent_info("/root/p/c1/kid"),
            agent_info("/root/other"),
        ],
        profiles: vec![agent_info("researcher"), agent_info("developer")],
        nodes: vec!["a".into(), "b".into()],
        edge_count: 3,
        is_cyclic: false,
        roots: vec!["a".into()],
        leaves: vec!["b".into()],
        rounds: 7,
        is_finished: false,
        termination: "MaxRounds { max: 99 }".into(),
    }
}

/// `list_agents` answers with exactly the caller's one-hop neighborhood
/// (direct parent + direct children, self excluded) and never leaks the
/// host's global topology fields.
#[tokio::test]
async fn list_agents_returns_only_visible_agents_and_no_topology() {
    let (control, mut cmd_rx) = test_control();
    let tool = ListAgentsTool {
        control,
        self_path: agentik_types::AgentPath::try_from("/root/p/c1").unwrap(),
    };

    let task = tokio::spawn(async move { tool.run(ListAgentsInput {}).await });
    match cmd_rx.recv().await.unwrap() {
        HostCommand::GetStatus { reply_tx } => {
            let _ = reply_tx.send(synthetic_status());
        }
        _ => panic!("expected GetStatus, got a different command variant"),
    }
    let result = task.await.unwrap().unwrap();
    let payload = match result.content {
        agentik_sdk::types::ToolResultContent::Json(value) => value,
        other => panic!("expected JSON content, got {other:?}"),
    };

    let paths: Vec<&str> = payload["agents"]
        .as_array()
        .expect("agents array")
        .iter()
        .map(|a| a["path"].as_str().expect("path"))
        .collect();
    // From /root/p/c1: parent /root/p + children /root/p/c1/gc and
    // /root/p/c1/kid. Self, the sibling branch /root/other, and the
    // grandchild /root/p/c1/gc/deeper would be excluded.
    assert_eq!(
        paths,
        vec!["/root/p", "/root/p/c1/gc", "/root/p/c1/kid"],
        "{payload}"
    );
    // Global topology fields must not be exposed to agents.
    for key in ["nodes", "edge_count", "rounds", "termination", "is_cyclic"] {
        assert!(payload.get(key).is_none(), "leaked `{key}`: {payload}");
    }
    assert!(payload["profiles"].is_array(), "{payload}");
}

/// Textual view of a `ToolResult`'s content for assertion purposes.
fn text_content(result: &agentik_sdk::types::ToolResult) -> String {
    match &result.content {
        agentik_sdk::types::ToolResultContent::Text(text) => text.clone(),
        agentik_sdk::types::ToolResultContent::Json(value) => value.to_string(),
        agentik_sdk::types::ToolResultContent::Blocks(blocks) => format!("{blocks:?}"),
    }
}

/// `get_agent_info` answers for visible agents and returns the
/// not-found message for invisible ones (no existence leak).
#[tokio::test]
async fn get_agent_info_denies_invisible_agents() {
    let self_path = agentik_types::AgentPath::try_from("/root/p/c1").unwrap();

    // Invisible target.
    let (control, mut cmd_rx) = test_control();
    let tool = GetAgentInfoTool {
        control,
        self_path: self_path.clone(),
    };
    let input = GetAgentInfoInput {
        agent_name: "other".into(),
    };
    let task = tokio::spawn(async move { tool.run(input).await });
    match cmd_rx.recv().await.unwrap() {
        HostCommand::GetAgentInfo { reply_tx, .. } => {
            let _ = reply_tx.send(Some(agent_info("/root/other")));
        }
        _ => panic!("expected GetAgentInfo command"),
    }
    let result = task.await.unwrap().unwrap();
    let text = text_content(&result);
    assert!(text.contains("not found"), "{text}");

    // Visible target (direct parent).
    let (control, mut cmd_rx) = test_control();
    let tool = GetAgentInfoTool { control, self_path };
    let input = GetAgentInfoInput {
        agent_name: "p".into(),
    };
    let task = tokio::spawn(async move { tool.run(input).await });
    match cmd_rx.recv().await.unwrap() {
        HostCommand::GetAgentInfo { reply_tx, .. } => {
            let _ = reply_tx.send(Some(agent_info("/root/p")));
        }
        _ => panic!("expected GetAgentInfo command"),
    }
    let result = task.await.unwrap().unwrap();
    let text = text_content(&result);
    assert!(!text.contains("not found"), "{text}");
}

#[tokio::test]
async fn concurrent_delegation_commands_have_distinct_ids() {
    let (control, mut cmd_rx) = test_control();
    let first_control = control.clone();
    let first = tokio::spawn(async move {
        first_control
            .delegate_tracked(
                "/root/researcher",
                "first",
                Some("/root/caller".into()),
                uuid::Uuid::new_v4(),
                None,
            )
            .await
    });
    let second = tokio::spawn(async move {
        control
            .delegate_tracked(
                "/root/researcher",
                "second",
                Some("/root/caller".into()),
                uuid::Uuid::new_v4(),
                None,
            )
            .await
    });

    let mut ids = Vec::new();
    for _ in 0..2 {
        match cmd_rx.recv().await.unwrap() {
            HostCommand::Delegate { delegation_id, .. } => ids.push(delegation_id),
            _ => panic!("expected delegate command"),
        }
    }
    drop(cmd_rx);
    let _ = tokio::join!(first, second);
    assert_ne!(ids[0], ids[1]);
}
