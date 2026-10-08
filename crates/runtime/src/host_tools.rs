//! Agent tools for controlling the multi-agent host.
//!
//! Each tool wraps a [`HostControl`] command and exposes it to the LLM as
//! a callable function. Agents use these tools to spawn their own child
//! agents, delegate tasks to them, and manage/query the agents visible to
//! them. Inter-agent communication is delegation-only and strictly
//! parent→direct-child; every tool below enforces that one-hop scope.

use agentik_core::tools::{ToolContext, ToolFunction, ToolRegistration};
use agentik_proc::tool;
use agentik_sdk::types::ToolResult;
use async_trait::async_trait;

use crate::control::HostControl;

/// One-hop visibility: an agent sees itself, its direct parent, and its
/// direct children — nothing else.
fn visible_from(caller: &agentik_types::AgentPath, other: &agentik_types::AgentPath) -> bool {
    *other == *caller || other.is_direct_child_of(caller) || caller.is_direct_child_of(other)
}

/// Build the full set of host control tools for an agent.
/// Returns an empty vec if `control` is `None`.
///
/// `self_path` is the calling agent's own hierarchical path. It is used to:
/// - Scope `list_agents` / `get_agent_info` / `get_agent_history` /
///   `route_task` results to the agents visible to the caller (self,
///   direct parent, direct children).
/// - Derive child paths when the agent spawns sub-agents
///   (`self_path.join("worker")` → `/root/agent/worker`).
pub fn host_tools(
    control: Option<HostControl>,
    self_path: &agentik_types::AgentPath,
    caller_kind: agentik_core::AgentKind,
) -> Vec<ToolRegistration> {
    let Some(ctrl) = control else {
        return vec![];
    };
    let self_path = self_path.clone();
    vec![
        ToolRegistration::from(SpawnAgentTool {
            control: ctrl.clone(),
            caller_path: self_path.clone(),
            caller_kind,
        }),
        ToolRegistration::from(DelegateToTool {
            control: ctrl.clone(),
            caller_path: self_path.as_str().to_string(),
        }),
        ToolRegistration::from(RouteTaskTool {
            control: ctrl.clone(),
            self_path: self_path.clone(),
        }),
        ToolRegistration::from(GetAgentInfoTool {
            control: ctrl.clone(),
            self_path: self_path.clone(),
        }),
        ToolRegistration::from(ListAgentsTool {
            control: ctrl.clone(),
            self_path: self_path.clone(),
        }),
        ToolRegistration::from(ListDelegationsTool {
            control: ctrl.clone(),
            caller_path: self_path.as_str().to_string(),
        }),
        ToolRegistration::from(GetAgentHistoryTool {
            control: ctrl.clone(),
            self_path: self_path.clone(),
        }),
        ToolRegistration::from(ShutdownAgentTool {
            control: ctrl.clone(),
            self_path: self_path.clone(),
        }),
        ToolRegistration::from(InterruptAgentTool {
            control: ctrl.clone(),
            self_path: self_path.clone(),
        }),
    ]
}

// ═══════════════════════════════════════════════════════════════════════
// Spawn Agent
// ═══════════════════════════════════════════════════════════════════════

#[tool(
    name = "spawn_agent",
    description = "Spawn a new child agent and register it with the host. \
                   The child's path is automatically derived from your path \
                   (e.g. spawning 'worker' becomes /root/you/worker). \
                   Use profile_segment='researcher' for data analysis, DAG \
                   execution, and interpretation. Use profile_segment='developer' \
                   only to implement, validate, install, or repair a plugin/node. \
                   Do not use developer as an execution service for research data; \
                   omit profile_segment to reuse your own role."
)]
struct SpawnAgentInput {
    /// Short name for the new agent (a path segment, e.g. `worker`, `analyst`).
    /// Must be lowercase `[a-z0-9_]`, 1-32 chars.
    agent_name: String,
    /// Role to instantiate: `researcher`, `developer`, or omitted to reuse \
    /// your own role.
    profile_segment: Option<String>,
}

struct SpawnAgentTool {
    control: HostControl,
    caller_path: agentik_types::AgentPath,
    caller_kind: agentik_core::AgentKind,
}

#[async_trait]
impl ToolFunction for SpawnAgentTool {
    type Input = SpawnAgentInput;

    async fn run(
        &self,
        input: SpawnAgentInput,
    ) -> Result<ToolResult, agentik_core::tools::ToolError> {
        match self
            .control
            .spawn_agent(
                &input.agent_name,
                &self.caller_path,
                self.caller_kind,
                input.profile_segment.as_deref(),
            )
            .await
        {
            Ok(path) => Ok(ToolResult::success(format!(
                "Agent at path `{path}` spawned and registered."
            ))),
            Err(e) => Ok(ToolResult::success(format!("Spawn failed: {e}"))),
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Delegate To Agent (request-response, background async)
// ═══════════════════════════════════════════════════════════════════════

#[tool(
    name = "delegate_to",
    description = "Delegate a task to one of your direct child agents. Runs in the \
                   background and returns a task number (#N) immediately. Use \
                   `wait_task` with the task number to block until the result is \
                   ready, or `view_task_results` to poll for the output. Multiple \
                   delegates can run concurrently. The target agent's COMPLETE \
                   response becomes the task's result. For \
                   Researcher-to-Developer work, delegate capability \
                   implementation only: provide contracts and synthetic \
                   fixtures, never a research dataset or a request to \
                   run/interpret an analysis. You may delegate only to your \
                   own direct child agents (spawned via spawn_agent). Sibling, \
                   parent, and cross-branch delegation is rejected — spawn the \
                   agent you need first."
)]
struct DelegateToInput {
    /// Name of the target agent: a direct child's short name (e.g. \
    /// "worker") or full path (e.g. "/root/you/worker").
    agent_name: String,
    /// The task to send. For node/plugin development, include the requested \
    /// interface, acceptance criteria, and synthetic fixture instead of real \
    /// research data.
    task: String,
}

struct DelegateToTool {
    control: HostControl,
    caller_path: String,
}

#[async_trait]
impl ToolFunction for DelegateToTool {
    type Input = DelegateToInput;

    /// Async — the caller is notified when the target agent finishes; the
    /// full response is pulled on demand via `view_task_results`.
    fn execution_mode(&self) -> agentik_core::tools::ExecutionMode {
        agentik_core::tools::ExecutionMode::Async
    }

    /// 24-hour timeout — delegated agents may run long analyses.
    fn timeout_seconds(&self) -> u64 {
        86400
    }

    async fn run(
        &self,
        input: DelegateToInput,
    ) -> Result<ToolResult, agentik_core::tools::ToolError> {
        match self
            .control
            .delegate(&self.caller_path, &input.agent_name, input.task)
            .await
        {
            Some(response) => Ok(ToolResult::success(response)),
            None => Ok(ToolResult::success(format!(
                "Delegation to '{}' failed — agent not found or host channel closed.",
                input.agent_name
            ))),
        }
    }

    async fn execute_with_context(
        &self,
        input: serde_json::Value,
        ctx: &ToolContext,
    ) -> Result<ToolResult, agentik_core::tools::ToolError> {
        let input: DelegateToInput = serde_json::from_value(input)?;
        let delegation_id = uuid::Uuid::new_v4();
        ctx.set_metadata(serde_json::json!({
            "kind": "delegation",
            "delegation_id": delegation_id,
            "caller_agent": self.caller_path,
            "target_agent": input.agent_name,
            "task": input.task,
        }));
        ctx.emit(
            agentik_core::tools::ProgressRecord::new("delegation")
                .status("pending")
                .message(format!("target={}", input.agent_name)),
        );
        match self
            .control
            .delegate_tracked(
                &input.agent_name,
                input.task,
                Some(self.caller_path.clone()),
                delegation_id,
                ctx.output.clone(),
            )
            .await
        {
            Some(response) => Ok(ToolResult::success(response)),
            None => Ok(ToolResult::success(format!(
                "Delegation to '{}' failed — agent not found or host channel closed.",
                input.agent_name
            ))),
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Route Task — find the best agent for a task
// ═══════════════════════════════════════════════════════════════════════

#[tool(
    name = "route_task",
    description = "Find the best agent for a task based on capability matching. \
                   Describe what you need done and this tool returns the recommended \
                   agent name, match reason, and all candidates with scores. \
                   Candidates are your direct children plus the spawnable \
                   researcher/developer profiles. Use the returned agent name with \
                   delegate_to (an existing child) or spawn_agent (a role profile). \
                   Researcher owns data analysis and DAG execution; Developer owns \
                   plugin/node implementation and focused validation."
)]
struct RouteTaskInput {
    /// Natural language description of the task to route.
    description: String,
}

struct RouteTaskTool {
    control: HostControl,
    self_path: agentik_types::AgentPath,
}

#[async_trait]
impl ToolFunction for RouteTaskTool {
    type Input = RouteTaskInput;

    async fn run(
        &self,
        input: RouteTaskInput,
    ) -> Result<ToolResult, agentik_core::tools::ToolError> {
        match self
            .control
            .route_task(&input.description, self.self_path.as_str())
            .await
        {
            Some(result) => Ok(ToolResult::success_json(
                serde_json::to_value(&result).unwrap_or_default(),
            )),
            None => Ok(ToolResult::success("Routing failed — host unavailable.")),
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Get Agent Info — query a specific agent's capabilities
// ═══════════════════════════════════════════════════════════════════════

#[tool(
    name = "get_agent_info",
    description = "Get detailed capability info for a visible agent (yourself, your \
                   direct parent, or one of your direct children): summary, tags, \
                   expertise areas, and available tools. Also answers for role \
                   profiles (e.g. 'researcher') to inspect what a spawned child \
                   of that kind can do."
)]
struct GetAgentInfoInput {
    /// Name of the agent to query.
    agent_name: String,
}

struct GetAgentInfoTool {
    control: HostControl,
    self_path: agentik_types::AgentPath,
}

#[async_trait]
impl ToolFunction for GetAgentInfoTool {
    type Input = GetAgentInfoInput;

    async fn run(
        &self,
        input: GetAgentInfoInput,
    ) -> Result<ToolResult, agentik_core::tools::ToolError> {
        match self.control.get_agent_info(&input.agent_name).await {
            Some(info) => {
                // Visibility gate: only parent/children (and self) are
                // answerable. Non-`/root` paths are the static role-profile
                // fallback and are always allowed. Invisible agents get the
                // same message as not-found so callers cannot probe the
                // registry.
                if let Ok(path) = agentik_types::AgentPath::try_from(info.path.as_str()) {
                    if !visible_from(&self.self_path, &path) {
                        return Ok(ToolResult::success(format!(
                            "Agent '{}' not found.",
                            input.agent_name
                        )));
                    }
                }
                Ok(ToolResult::success_json(
                    serde_json::to_value(&info).unwrap_or_default(),
                ))
            }
            None => Ok(ToolResult::success(format!(
                "Agent '{}' not found.",
                input.agent_name
            ))),
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════
// List Agents / Network Status
// ═══════════════════════════════════════════════════════════════════════

#[tool(
    name = "list_agents",
    description = "List the agents visible to you — your direct parent (if any) and \
                   your direct children — plus the spawnable role profiles, as JSON. \
                   Each agent entry includes both `name` (short name) and `path` \
                   (full hierarchical path). Use the path with `delegate_to` when \
                   names are ambiguous."
)]
struct ListAgentsInput {}

struct ListAgentsTool {
    control: HostControl,
    self_path: agentik_types::AgentPath,
}

#[async_trait]
impl ToolFunction for ListAgentsTool {
    type Input = ListAgentsInput;

    async fn run(
        &self,
        _input: ListAgentsInput,
    ) -> Result<ToolResult, agentik_core::tools::ToolError> {
        match self.control.get_status().await {
            Some(status) => {
                // One-hop visibility: parent + direct children (self
                // excluded). Siblings and other branches are invisible, and
                // the host's global topology fields are never exposed.
                let visible: Vec<_> = status
                    .agents
                    .into_iter()
                    .filter(|a| {
                        agentik_types::AgentPath::try_from(a.path.as_str()).is_ok_and(|p| {
                            p != self.self_path && visible_from(&self.self_path, &p)
                        })
                    })
                    .collect();
                let mut profiles = status.profiles;
                profiles.retain(|p| p.name != self.self_path.name());
                Ok(ToolResult::success_json(serde_json::json!({
                    "agents": visible,
                    "profiles": profiles,
                })))
            }
            None => Ok(ToolResult::success("Failed to get host status.")),
        }
    }
}

#[tool(
    name = "list_delegations",
    description = "List your agent-to-agent delegations with stable IDs, target \
                   agent, turn ID, terminal status, and response. Filter by \
                   target agent or status; omit filters to list all of your \
                   delegations, newest first."
)]
struct ListDelegationsInput {
    /// Full or short target-agent name.
    target_agent: Option<String>,
    /// `pending`, `running`, `completed`, `interrupted`, or `failed`.
    status: Option<String>,
}

struct ListDelegationsTool {
    control: HostControl,
    caller_path: String,
}

#[async_trait]
impl ToolFunction for ListDelegationsTool {
    type Input = ListDelegationsInput;

    async fn run(
        &self,
        input: ListDelegationsInput,
    ) -> Result<ToolResult, agentik_core::tools::ToolError> {
        match self
            .control
            .list_delegations(
                Some(&self.caller_path),
                input.target_agent.as_deref(),
                input.status.as_deref(),
            )
            .await
        {
            Some(delegations) => Ok(ToolResult::success_json(
                serde_json::to_value(delegations).unwrap_or_default(),
            )),
            None => Ok(ToolResult::success(
                "Failed to query delegations — host unavailable.",
            )),
        }
    }
}

#[tool(
    name = "get_agent_history",
    description = "Read the persisted conversation history of a visible agent \
                   (yourself, your direct parent, or one of your direct children). \
                   Returns the most recent messages for the agent, which can be used \
                   to inspect what a delegated task actually did."
)]
struct GetAgentHistoryInput {
    /// Full or short name of the target agent.
    agent_name: String,
    /// Maximum number of messages to return. Default 20; clamped to 100.
    #[default = 20]
    limit: Option<usize>,
}

struct GetAgentHistoryTool {
    control: HostControl,
    self_path: agentik_types::AgentPath,
}

#[async_trait]
impl ToolFunction for GetAgentHistoryTool {
    type Input = GetAgentHistoryInput;

    async fn run(
        &self,
        input: GetAgentHistoryInput,
    ) -> Result<ToolResult, agentik_core::tools::ToolError> {
        let limit = input.limit.unwrap_or(20).clamp(1, 100);
        match self.control.agent_history(&input.agent_name, limit).await {
            Some(history) => {
                // Visibility gate on the resolved target path. A history
                // whose path cannot be parsed is treated as invisible.
                let visible = agentik_types::AgentPath::try_from(history.agent_path.as_str())
                    .is_ok_and(|p| visible_from(&self.self_path, &p));
                if !visible {
                    return Ok(ToolResult::success(format!(
                        "Agent '{}' not found or not visible to you.",
                        input.agent_name
                    )));
                }
                Ok(ToolResult::success_json(
                    serde_json::to_value(history).unwrap_or_default(),
                ))
            }
            None => Ok(ToolResult::success(
                "Failed to read agent history — host unavailable.",
            )),
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Shutdown Agent
// ═══════════════════════════════════════════════════════════════════════

#[tool(
    name = "shutdown_agent",
    description = "Shut down one of your direct child agents and remove it from \
                   the registry. The child's process is terminated — it cannot \
                   receive any more messages. Use this when a child is no longer \
                   needed (e.g. its delegated analysis session is complete). \
                   For a softer cancellation that lets the child accept a \
                   new message afterwards, use interrupt_agent instead."
)]
struct ShutdownAgentInput {
    /// Name of the child agent to shut down. Accepts a short name (e.g. \
    /// "worker") or full path (e.g. "/root/you/worker").
    agent_name: String,
}

struct ShutdownAgentTool {
    control: HostControl,
    self_path: agentik_types::AgentPath,
}

#[async_trait]
impl ToolFunction for ShutdownAgentTool {
    type Input = ShutdownAgentInput;

    async fn run(
        &self,
        input: ShutdownAgentInput,
    ) -> Result<ToolResult, agentik_core::tools::ToolError> {
        match self
            .control
            .shutdown_child_agent(self.self_path.as_str(), &input.agent_name)
            .await
        {
            Some(Ok(())) => Ok(ToolResult::success(format!(
                "Agent '{}' shutdown requested.",
                input.agent_name
            ))),
            Some(Err(e)) => Ok(ToolResult::success(format!("Shutdown failed: {e}"))),
            None => Ok(ToolResult::success(
                "Shutdown failed: host unavailable (runtime shut down or unresponsive).",
            )),
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Interrupt Agent — cancel current turn, keep agent alive
// ═══════════════════════════════════════════════════════════════════════

#[tool(
    name = "interrupt_agent",
    description = "Interrupt one of your direct child agents' CURRENT turn without \
                   shutting it down. The child emits LifecycleChanged(Cancelled), \
                   aborts any in-flight tool calls, and remains registered — a \
                   follow-up delegate_to will start a fresh turn on the same \
                   session. Use this when:\n\
                   - You delegated a long task and want to cancel it (e.g. wrong \
                     agent selected, task is taking too long, etc.)\n\
                   - The child is stuck in a retry loop and you want to break out.\n\
                   - You want to redirect the child's work mid-turn.\n\
                   For full agent shutdown (removes from registry), use \
                   shutdown_agent instead."
)]
struct InterruptAgentInput {
    /// Name of the child agent to interrupt. Accepts a short name (e.g. \
    /// "worker") or full path.
    agent_name: String,
    /// Optional human-readable reason logged alongside the cancel \
    /// event. Useful when debugging why an agent got interrupted.
    #[serde(default)]
    reason: Option<String>,
}

struct InterruptAgentTool {
    control: HostControl,
    self_path: agentik_types::AgentPath,
}

#[async_trait]
impl ToolFunction for InterruptAgentTool {
    type Input = InterruptAgentInput;

    /// Synchronous — interrupt is a fast operation. The child's
    /// cancel_token is cancelled, which propagates immediately to any
    /// in-flight LLM request or tool execution. The child then emits
    /// `LifecycleChanged(Cancelled)` through the event stream; if you're
    /// tracking the result, follow up with `view_task_results` to see the
    /// Cancelled status.
    /// Sync (default) — returns as soon as the host confirms the cancel
    /// command (or denies a non-child target).
    /// 1 hour cap — interrupt itself should be near-instant; the cap
    /// only matters if the host command channel is jammed.
    fn timeout_seconds(&self) -> u64 {
        3600
    }

    async fn run(
        &self,
        input: InterruptAgentInput,
    ) -> Result<ToolResult, agentik_core::tools::ToolError> {
        let reason = input
            .reason
            .as_deref()
            .unwrap_or("user-requested interrupt");
        tracing::info!(
            agent = %input.agent_name,
            reason = %reason,
            "interrupt_agent: cancelling current turn"
        );
        match self
            .control
            .interrupt_child_agent(self.self_path.as_str(), &input.agent_name)
            .await
        {
            Some(Ok(())) => Ok(ToolResult::success(format!(
                "Interrupt requested for agent '{}'. \
                 The current turn will be cancelled; the agent remains \
                 available for new messages. Reason: {}",
                input.agent_name, reason
            ))),
            Some(Err(e)) => Ok(ToolResult::success(format!("Interrupt failed: {e}"))),
            None => Ok(ToolResult::success(
                "Interrupt failed: host unavailable (runtime shut down or unresponsive).",
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    //! Struct-level tests that require access to the private tool structs
    //! (not accessible from `host.rs`), plus policy guards over the tool
    //! registry and the visibility filtering of the read tools.

    use super::*;
    use crate::control::HostCommand;
    use crate::host::HostEvent;
    use agentik_core::tools::{ExecutionMode, ToolFunction, ToolRegistration};

    fn test_control() -> (HostControl, tokio::sync::mpsc::UnboundedReceiver<HostCommand>) {
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
                "route_task",
                "shutdown_agent",
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
            status: crate::control::AgentStatus::Idle,
            last_event: None,
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
        let tool = GetAgentInfoTool {
            control,
            self_path,
        };
        let input = GetAgentInfoInput { agent_name: "p".into() };
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
}
