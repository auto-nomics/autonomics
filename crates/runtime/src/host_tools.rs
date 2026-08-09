//! Agent tools for controlling the multi-agent host.
//!
//! Each tool wraps a [`HostControl`] command and exposes it to the LLM as
//! a callable function. Agents use these tools to spawn peers, manage
//! topology, send messages, and query system status.
//!
//! Some tools (connect_agents, disconnect_agents, reset_network) are
//! currently disabled — multi-agent cooperation is delegate-driven.
//! Their struct definitions are kept for future use.

#![allow(dead_code)]

use agentik_core::tools::{ToolFunction, ToolRegistration};
use agentik_network::{EdgeTrigger, TerminationSpec};
use agentik_proc::tool;
use agentik_sdk::types::ToolResult;
use async_trait::async_trait;

use crate::control::HostControl;

/// Build the full set of host control tools for an agent.
/// Returns an empty vec if `control` is `None`.
///
/// `self_path` is the calling agent's own hierarchical path. It is used to:
/// - Exclude the agent from its own `list_agents` / `route_task` results
///   (preventing self-delegation).
/// - Derive child paths when the agent spawns sub-agents
///   (`self_path.join("worker")` → `/root/agent/worker`).
pub fn host_tools(
    control: Option<HostControl>,
    self_path: &agentik_types::AgentPath,
    caller_profile_path: &str,
) -> Vec<ToolRegistration> {
    let Some(ctrl) = control else {
        return vec![];
    };
    let self_path = self_path.clone();
    vec![
        ToolRegistration::from(SpawnAgentTool {
            control: ctrl.clone(),
            caller_path: self_path.clone(),
            caller_profile_path: caller_profile_path.into(),
        }),
        ToolRegistration::from(DeriveProfileTool {
            control: ctrl.clone(),
            caller_profile_path: caller_profile_path.into(),
        }),
        ToolRegistration::from(DelegateToTool {
            control: ctrl.clone(),
        }),
        ToolRegistration::from(SendMessageTool {
            control: ctrl.clone(),
        }),
        ToolRegistration::from(RouteTaskTool {
            control: ctrl.clone(),
            self_path: self_path.clone(),
        }),
        ToolRegistration::from(GetAgentInfoTool {
            control: ctrl.clone(),
        }),
        ToolRegistration::from(ListAgentsTool {
            control: ctrl.clone(),
            self_path: self_path.clone(),
        }),
        ToolRegistration::from(WaitAgentTool {
            control: ctrl.clone(),
        }),
        // ── Topology-edge tools disabled ──
        // Multi-agent cooperation is now fully delegate-driven. Agents
        // discover peers via route_task/get_agent_info and delegate via
        // delegate_to. No explicit topology graph needed.
        // ToolRegistration::from(ConnectAgentsTool { control: ctrl.clone() }),
        // ToolRegistration::from(DisconnectAgentsTool { control: ctrl.clone() }),
        ToolRegistration::from(SetTerminationTool {
            control: ctrl.clone(),
        }),
        ToolRegistration::from(GetNetworkStatusTool {
            control: ctrl.clone(),
        }),
        ToolRegistration::from(ShutdownAgentTool {
            control: ctrl.clone(),
        }),
        ToolRegistration::from(InterruptAgentTool {
            control: ctrl.clone(),
        }),
        // ToolRegistration::from(ResetNetworkTool { control: ctrl.clone() }),
        ToolRegistration::from(InjectPromptsTool { control: ctrl }),
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
                   The agent will be created from a profile — either a child \
                   of your own profile, a root-level profile, or your own \
                   profile if no segment is specified."
)]
struct SpawnAgentInput {
    /// Short name for the new agent (a path segment, e.g. `worker`, `analyst`).
    /// Must be lowercase `[a-z0-9_]`, 1-32 chars.
    agent_name: String,
    /// Profile to instantiate. If omitted, reuses your own profile. \
    /// If a single segment (e.g. `genomics`), looks up a child profile \
    /// relative to your profile, falling back to root-level. \
    /// If a multi-segment path (e.g. `researcher/genomics`), treated as \
    /// an absolute profile path.
    profile_segment: Option<String>,
}

struct SpawnAgentTool {
    control: HostControl,
    caller_path: agentik_types::AgentPath,
    caller_profile_path: String,
}

#[async_trait]
impl ToolFunction for SpawnAgentTool {
    type Input = SpawnAgentInput;

    async fn run(
        &self,
        input: SpawnAgentInput,
    ) -> Result<ToolResult, agentik_core::tools::ToolError> {
        match self.control.spawn_agent(
            &input.agent_name,
            &self.caller_path,
            &self.caller_profile_path,
            input.profile_segment.as_deref(),
        ).await
        {
            Ok(path) => Ok(ToolResult::success(format!(
                "Agent at path `{path}` spawned and registered."
            ))),
            Err(e) => Ok(ToolResult::success(format!("Spawn failed: {e}"))),
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Derive Profile (dynamic child profile creation)
// ═══════════════════════════════════════════════════════════════════════

#[tool(
    name = "derive_profile",
    description = "Derive a specialized child profile from your own profile. \
                   The child inherits all your capabilities unless explicitly \
                   overridden. The child's path is your_profile_path/segment. \
                   After derivation, you can spawn agents from the new profile \
                   using spawn_agent with the segment as profile_segment."
)]
struct DeriveProfileInput {
    /// Segment name for the child profile (e.g. `genomics`, `mr_analysis`).
    /// Must be lowercase `[a-z0-9_]`, 1-32 chars.
    segment: String,
    /// Human-readable description of this specialized role.
    description: Option<String>,
    /// Override the agent identity prompt.
    agent_identity: Option<String>,
    /// Override tool capability flags (None = inherit parent).
    enable_bibliography: Option<bool>,
    enable_writing: Option<bool>,
    enable_opengwas: Option<bool>,
    enable_opentargets: Option<bool>,
    enable_gwascatalog: Option<bool>,
    enable_iceberg: Option<bool>,
    enable_dag_history: Option<bool>,
}

struct DeriveProfileTool {
    control: HostControl,
    caller_profile_path: String,
}

#[async_trait]
impl ToolFunction for DeriveProfileTool {
    type Input = DeriveProfileInput;

    async fn run(
        &self,
        input: DeriveProfileInput,
    ) -> Result<ToolResult, agentik_core::tools::ToolError> {
        let overrides = agentik_core::ProfileOverrides {
            description: input.description,
            agent_identity: input.agent_identity,
            enable_bibliography: input.enable_bibliography,
            enable_writing: input.enable_writing,
            enable_opengwas: input.enable_opengwas,
            enable_opentargets: input.enable_opentargets,
            enable_gwascatalog: input.enable_gwascatalog,
            enable_iceberg: input.enable_iceberg,
            enable_dag_history: input.enable_dag_history,
            ..Default::default()
        };
        match self
            .control
            .derive_profile(&self.caller_profile_path, &input.segment, overrides)
            .await
        {
            Ok(path) => Ok(ToolResult::success(format!(
                "Derived profile `{path}`. Use spawn_agent with \
                 profile_segment=\"{}\" to instantiate.",
                input.segment
            ))),
            Err(e) => Ok(ToolResult::success(format!(
                "Derive failed: {e}"
            ))),
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Delegate To Agent (request-response, background async)
// ═══════════════════════════════════════════════════════════════════════

#[tool(
    name = "delegate_to",
    description = "Delegate a task to another agent and wait for its full response. \
                   The target agent processes the message and its complete output is \
                   returned as this tool's result. Runs in the background — use \
                   wait_task / view_task_results to retrieve the response. \
                   Multiple delegates can run concurrently."
)]
struct DelegateToInput {
    /// Name of the target agent. Accepts a short name (e.g. "researcher")
    /// or full path (e.g. "/root/researcher/worker").
    agent_name: String,
    /// The task or question to send to the target agent.
    task: String,
}

struct DelegateToTool {
    control: HostControl,
}

#[async_trait]
impl ToolFunction for DelegateToTool {
    type Input = DelegateToInput;

    /// sync_seconds = 0 → immediately goes to background execution.
    /// The agent can continue other work and retrieve the result via
    /// wait_task / view_task_results.
    fn sync_seconds(&self) -> u64 {
        0
    }

    /// 24-hour timeout — delegated agents may run long analyses.
    fn timeout_seconds(&self) -> u64 {
        86400
    }

    async fn run(
        &self,
        input: DelegateToInput,
    ) -> Result<ToolResult, agentik_core::tools::ToolError> {
        match self.control.delegate(&input.agent_name, input.task).await {
            Some(response) => Ok(ToolResult::success(response)),
            None => Ok(ToolResult::success(format!(
                "Delegation to '{}' failed — agent not found or host channel closed.",
                input.agent_name
            ))),
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Send Message — fire-and-forget inter-agent message (Phase 5)
// ═══════════════════════════════════════════════════════════════════════

#[tool(
    name = "send_message",
    description = "Send a fire-and-forget message to another agent. Unlike delegate_to, \
                   this does NOT wait for the target's response — the message is \
                   delivered and you continue immediately. Use this when:\n\
                   - You want to notify another agent of something without needing a reply.\n\
                   - You want to kick off background work on another agent and check \
                     results later via wait_agent or get_agent_info.\n\
                   - You need to send multiple messages to different agents in parallel.\n\
                   The target agent processes the message in its own turn. If it is \
                   currently busy, the message is queued and handled on its next turn."
)]
struct SendMessageInput {
    /// Name of the target agent. Accepts a short name (e.g. "researcher") \
    /// or full path (e.g. "/root/researcher/worker").
    agent_name: String,
    /// The message content to deliver.
    message: String,
}

struct SendMessageTool {
    control: HostControl,
}

#[async_trait]
impl ToolFunction for SendMessageTool {
    type Input = SendMessageInput;

    /// Synchronous fast-return — the tool completes as soon as the host
    /// confirms delivery (name resolution + channel send). No waiting
    /// for the target agent's response.
    fn sync_seconds(&self) -> u64 {
        30
    }

    /// 1-hour hard cap. Delivery is near-instant; the cap only matters
    /// if the host command channel is jammed.
    fn timeout_seconds(&self) -> u64 {
        3600
    }

    async fn run(
        &self,
        input: SendMessageInput,
    ) -> Result<ToolResult, agentik_core::tools::ToolError> {
        match self
            .control
            .send_message(&input.agent_name, input.message)
            .await
        {
            Some(Ok(())) => Ok(ToolResult::success(format!(
                "Message delivered to '{}'.",
                input.agent_name
            ))),
            Some(Err(e)) => Ok(ToolResult::success(format!(
                "send_message failed: {e}"
            ))),
            None => Ok(ToolResult::success(
                "send_message: host command channel closed (runtime shut down)",
            )),
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
                   Use the returned agent name with delegate_to."
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
            .route_task(&input.description, Some(self.self_path.as_str()))
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
    description = "Get detailed capability info for a specific agent: summary, tags, \
                   expertise areas, and available tools."
)]
struct GetAgentInfoInput {
    /// Name of the agent to query.
    agent_name: String,
}

struct GetAgentInfoTool {
    control: HostControl,
}

#[async_trait]
impl ToolFunction for GetAgentInfoTool {
    type Input = GetAgentInfoInput;

    async fn run(
        &self,
        input: GetAgentInfoInput,
    ) -> Result<ToolResult, agentik_core::tools::ToolError> {
        match self.control.get_agent_info(&input.agent_name).await {
            Some(info) => Ok(ToolResult::success_json(
                serde_json::to_value(&info).unwrap_or_default(),
            )),
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
    description = "List all registered agents (excluding yourself) and current \
                   network topology status as JSON. Each agent entry includes \
                   both `name` (short name) and `path` (full hierarchical path). \
                   Use the path with `delegate_to` when names are ambiguous."
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
            Some(mut status) => {
                // Exclude self from the agent list to prevent self-delegation.
                status.agents.retain(|a| a.path != self.self_path.as_str());
                status.profiles.retain(|p| p.name != self.self_path.name());
                Ok(ToolResult::success_json(
                    serde_json::to_value(&status).unwrap_or_default(),
                ))
            }
            None => Ok(ToolResult::success("Failed to get host status.")),
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Wait Agent — block until peer reaches Completed/Failed
// ═══════════════════════════════════════════════════════════════════════

/// Default wait timeout — 5 minutes. Long enough for most delegated
/// analyses, short enough that a runaway wait doesn't pin the caller
/// forever.
const DEFAULT_WAIT_TIMEOUT_MS: u64 = 300_000;

/// Minimum wait timeout — 1 second. Prevents tight-loop waits when
/// the LLM asks for `timeout_ms = 0`.
const MIN_WAIT_TIMEOUT_MS: u64 = 1_000;

/// Maximum wait timeout — 1 hour. Hard ceiling even if the LLM asks
/// for more. Past this, the caller should use multiple sequential
/// waits or `delegate_to` directly.
const MAX_WAIT_TIMEOUT_MS: u64 = 3_600_000;

#[tool(
    name = "wait_agent",
    description = "Block until the named agent reaches Completed or Failed status, \
                   or until the timeout elapses. Use after delegate_to when you \
                   want to wait for a background task's result instead of \
                   polling list_agents. Returns JSON with the final status, \
                   last event, and timed_out flag. Default timeout 5min, \
                   clamped to [1s, 1h]."
)]
struct WaitAgentInput {
    /// Name of the agent to wait for. Accepts a short name (e.g. \
    /// "researcher") or full path (e.g. "/root/researcher/worker").
    agent_name: String,
    /// How long to wait in milliseconds. Defaults to 300000 (5 min). \
    /// Clamped to [1000, 3600000] ([1s, 1h]).
    #[serde(default)]
    timeout_ms: Option<u64>,
}

struct WaitAgentTool {
    control: HostControl,
}

#[async_trait]
impl ToolFunction for WaitAgentTool {
    type Input = WaitAgentInput;

    /// Background execution — `delegate_to` style. The caller can fire
    /// wait_agent and continue other work, retrieving the result via
    /// `wait_task` / `view_task_results`. Matches codex's
    /// `multi_agents::wait_agent` which uses the same pattern.
    fn sync_seconds(&self) -> u64 {
        0
    }

    /// Hard cap 24 hours. The inner wait is clamped to 1 hour, so
    /// the 24h ceiling only matters if the host command channel itself
    /// is jammed — let the timeout fire rather than pin a tool slot.
    fn timeout_seconds(&self) -> u64 {
        86400
    }

    async fn run(
        &self,
        input: WaitAgentInput,
    ) -> Result<ToolResult, agentik_core::tools::ToolError> {
        let timeout_ms = input
            .timeout_ms
            .unwrap_or(DEFAULT_WAIT_TIMEOUT_MS)
            .clamp(MIN_WAIT_TIMEOUT_MS, MAX_WAIT_TIMEOUT_MS);

        match self.control.wait_agent(&input.agent_name, timeout_ms).await {
            Some(Ok(result)) => Ok(ToolResult::success_json(
                serde_json::to_value(&result).unwrap_or_default(),
            )),
            Some(Err(e)) => Ok(ToolResult::success(format!(
                "wait_agent failed: {e}"
            ))),
            None => Ok(ToolResult::success(
                "wait_agent: host command channel closed (runtime shut down)",
            )),
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Connect Agents (disabled — delegate-driven cooperation)
// ═══════════════════════════════════════════════════════════════════════

// ═══════════════════════════════════════════════════════════════════════

#[tool(
    name = "connect_agents",
    description = "Create a directed topology edge from one agent to another. \
                   When the source agent completes its turn (Done), its output \
                   is forwarded to the target agent."
)]
struct ConnectAgentsInput {
    /// Source agent name.
    from: String,
    /// Target agent name.
    to: String,
    /// Trigger type: "on_done" (fire when source completes) or \
    /// "on_pattern" (fire when source response contains pattern).
    trigger: String,
    /// Pattern for on_pattern trigger (ignored for on_done).
    #[serde(default)]
    pattern: Option<String>,
}

struct ConnectAgentsTool {
    control: HostControl,
}

#[async_trait]
impl ToolFunction for ConnectAgentsTool {
    type Input = ConnectAgentsInput;

    async fn run(
        &self,
        input: ConnectAgentsInput,
    ) -> Result<ToolResult, agentik_core::tools::ToolError> {
        let trigger = match input.trigger.as_str() {
            "on_done" | "done" => EdgeTrigger::OnDone,
            "on_pattern" | "pattern" => EdgeTrigger::OnPattern {
                pattern: input.pattern.unwrap_or_default(),
            },
            other => {
                return Ok(ToolResult::success(format!(
                    "Unknown trigger '{other}'. Use 'on_done' or 'on_pattern'."
                )));
            }
        };
        self.control.connect(&input.from, &input.to, trigger);
        Ok(ToolResult::success(format!(
            "Connected {} → {}.",
            input.from, input.to
        )))
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Disconnect Agents
// ═══════════════════════════════════════════════════════════════════════

#[tool(
    name = "disconnect_agents",
    description = "Remove all topology edges from one agent to another."
)]
struct DisconnectAgentsInput {
    from: String,
    to: String,
}

struct DisconnectAgentsTool {
    control: HostControl,
}

#[async_trait]
impl ToolFunction for DisconnectAgentsTool {
    type Input = DisconnectAgentsInput;

    async fn run(
        &self,
        input: DisconnectAgentsInput,
    ) -> Result<ToolResult, agentik_core::tools::ToolError> {
        self.control.disconnect(&input.from, &input.to);
        Ok(ToolResult::success(format!(
            "Disconnected {} → {}.",
            input.from, input.to
        )))
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Set Termination
// ═══════════════════════════════════════════════════════════════════════

#[tool(
    name = "set_termination",
    description = "Set the termination condition for the agent network. \
                   Format: 'max_rounds:N' (stop after N node completions), \
                   'condition:node:pattern' (stop when node response contains pattern), \
                   'any_node_done:node1,node2' (stop when any listed node completes)."
)]
struct SetTerminationInput {
    /// Termination spec string, e.g. "max_rounds:10" or "condition:reviewer:ACCEPT".
    spec: String,
}

struct SetTerminationTool {
    control: HostControl,
}

#[async_trait]
impl ToolFunction for SetTerminationTool {
    type Input = SetTerminationInput;

    async fn run(
        &self,
        input: SetTerminationInput,
    ) -> Result<ToolResult, agentik_core::tools::ToolError> {
        use agentik_network::TerminationSpec;

        let spec = match parse_termination(&input.spec) {
            Ok(s) => s,
            Err(e) => return Ok(ToolResult::success(format!("Invalid spec: {e}"))),
        };
        self.control.set_termination(spec);
        Ok(ToolResult::success(format!(
            "Termination set to: {}",
            input.spec
        )))
    }
}

fn parse_termination(s: &str) -> Result<agentik_network::TerminationSpec, String> {
    let (kind, rest) = s
        .split_once(':')
        .ok_or_else(|| format!("expected 'kind:args', got '{s}'"))?;
    match kind {
        "max_rounds" => {
            let max: usize = rest.parse().map_err(|_| "max_rounds needs a number")?;
            Ok(TerminationSpec::MaxRounds { max })
        }
        "condition" => {
            let (node, pattern) = rest
                .split_once(':')
                .ok_or("condition needs 'node:pattern'")?;
            Ok(TerminationSpec::Condition {
                node: node.into(),
                pattern: pattern.into(),
            })
        }
        "any_node_done" => {
            let nodes: Vec<String> = rest.split(',').map(|s| s.trim().to_string()).collect();
            Ok(TerminationSpec::AnyNodeDone { nodes })
        }
        other => Err(format!("unknown termination kind: '{other}'")),
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Get Network Status (detailed JSON)
// ═══════════════════════════════════════════════════════════════════════

#[tool(
    name = "get_network_status",
    description = "Get detailed network topology status as JSON: agents, nodes, \
                   edges, cycles, roots, leaves, rounds, termination state."
)]
struct GetNetworkStatusInput {}

struct GetNetworkStatusTool {
    control: HostControl,
}

#[async_trait]
impl ToolFunction for GetNetworkStatusTool {
    type Input = GetNetworkStatusInput;

    async fn run(
        &self,
        _input: GetNetworkStatusInput,
    ) -> Result<ToolResult, agentik_core::tools::ToolError> {
        match self.control.get_status().await {
            Some(status) => Ok(ToolResult::success_json(
                serde_json::to_value(&status).unwrap_or_default(),
            )),
            None => Ok(ToolResult::success("Host unavailable.")),
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Shutdown Agent
// ═══════════════════════════════════════════════════════════════════════

#[tool(
    name = "shutdown_agent",
    description = "Shut down a named agent and remove it from the registry. \
                   The agent's process is terminated — it cannot receive \
                   any more messages. Use this when the agent is no longer \
                   needed (e.g. long-lived analysis session is complete). \
                   For a softer cancellation that lets the agent accept a \
                   new message afterwards, use interrupt_agent instead."
)]
struct ShutdownAgentInput {
    agent_name: String,
}

struct ShutdownAgentTool {
    control: HostControl,
}

#[async_trait]
impl ToolFunction for ShutdownAgentTool {
    type Input = ShutdownAgentInput;

    async fn run(
        &self,
        input: ShutdownAgentInput,
    ) -> Result<ToolResult, agentik_core::tools::ToolError> {
        self.control.shutdown_agent(&input.agent_name);
        Ok(ToolResult::success(format!(
            "Agent '{}' shutdown requested.",
            input.agent_name
        )))
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Interrupt Agent — cancel current turn, keep agent alive
// ═══════════════════════════════════════════════════════════════════════

#[tool(
    name = "interrupt_agent",
    description = "Interrupt the named agent's CURRENT turn without shutting it down. \
                   The agent emits LifecycleChanged(Cancelled), aborts any in-flight \
                   tool calls, and remains registered — a follow-up message via \
                   delegate_to / send_message will start a fresh turn on the same \
                   session. Use this when:\n\
                   - You delegated a long task and want to cancel it (e.g. wrong \
                     agent selected, task is taking too long, etc.)\n\
                   - The agent is stuck in a retry loop and you want to break out.\n\
                   - You want to redirect the agent's work mid-turn.\n\
                   For full agent shutdown (removes from registry), use \
                   shutdown_agent instead."
)]
struct InterruptAgentInput {
    /// Name of the agent to interrupt. Accepts a short name (e.g. \
    /// "researcher") or full path.
    agent_name: String,
    /// Optional human-readable reason logged alongside the cancel \
    /// event. Useful when debugging why an agent got interrupted.
    #[serde(default)]
    reason: Option<String>,
}

struct InterruptAgentTool {
    control: HostControl,
}

#[async_trait]
impl ToolFunction for InterruptAgentTool {
    type Input = InterruptAgentInput;

    /// Synchronous — interrupt is a fast fire-and-forget operation.
    /// The agent's cancel_token is cancelled, which propagates
    /// immediately to any in-flight LLM request or tool execution.
    /// The agent then emits `LifecycleChanged(Cancelled)` through the
    /// event stream; if you're tracking the result, follow up with
    /// `wait_agent` to see the Cancelled status.
    fn sync_seconds(&self) -> u64 {
        0
    }

    /// 1 hour cap — interrupt itself should be near-instant; the cap
    /// only matters if the host command channel is jammed.
    fn timeout_seconds(&self) -> u64 {
        3600
    }

    async fn run(
        &self,
        input: InterruptAgentInput,
    ) -> Result<ToolResult, agentik_core::tools::ToolError> {
        let reason = input.reason.as_deref().unwrap_or("user-requested interrupt");
        tracing::info!(
            agent = %input.agent_name,
            reason = %reason,
            "interrupt_agent: cancelling current turn"
        );
        self.control.cancel_agent(&input.agent_name);
        Ok(ToolResult::success(format!(
            "Interrupt requested for agent '{}'. \
             The current turn will be cancelled; the agent remains \
             available for new messages. Reason: {}",
            input.agent_name, reason
        )))
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Reset Network
// ═══════════════════════════════════════════════════════════════════════

#[tool(
    name = "reset_network",
    description = "Reset routing state (message buffers, round counts, finished flag) \
                   while keeping the topology graph intact. Use this to restart \
                   a run on the same topology."
)]
struct ResetNetworkInput {}

struct ResetNetworkTool {
    control: HostControl,
}

#[async_trait]
impl ToolFunction for ResetNetworkTool {
    type Input = ResetNetworkInput;

    async fn run(
        &self,
        _input: ResetNetworkInput,
    ) -> Result<ToolResult, agentik_core::tools::ToolError> {
        self.control.reset_run_state();
        Ok(ToolResult::success("Network routing state reset."))
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Inject Prompts
// ═══════════════════════════════════════════════════════════════════════

#[tool(
    name = "inject_prompts",
    description = "Inject initial prompts for all topology nodes that have them. \
                   Call this after building the topology to kick-start the network."
)]
struct InjectPromptsInput {}

struct InjectPromptsTool {
    control: HostControl,
}

#[async_trait]
impl ToolFunction for InjectPromptsTool {
    type Input = InjectPromptsInput;

    async fn run(
        &self,
        _input: InjectPromptsInput,
    ) -> Result<ToolResult, agentik_core::tools::ToolError> {
        self.control.inject_prompts();
        Ok(ToolResult::success("Initial prompts injected."))
    }
}

#[cfg(test)]
mod tests {
    //! Phase 5 — `SendMessageTool` struct-level tests that require access
    //! to the private tool struct (not accessible from `host.rs`).

    use super::*;
    use crate::control::HostCommand;
    use crate::host::HostEvent;
    use agentik_core::tools::ToolFunction;

    /// The `SendMessageTool` should be synchronous with a fast return
    /// (not background like `delegate_to` or `wait_agent`). This ensures
    /// the calling agent gets immediate delivery confirmation without
    /// occupying a background task slot.
    #[test]
    fn send_message_tool_is_synchronous_fast_return() {
        let (cmd_tx, _cmd_rx) = tokio::sync::mpsc::unbounded_channel::<HostCommand>();
        let (event_tx, _) = tokio::sync::broadcast::channel::<HostEvent>(1);
        let control = HostControl::new(cmd_tx, event_tx);
        let tool = SendMessageTool { control };

        // sync_seconds > 0 → synchronous (agent waits for completion).
        // Not 0 (which would mean background / fire-and-forget at the
        // tool framework level).
        assert!(
            tool.sync_seconds() > 0,
            "send_message must be synchronous, not background"
        );

        // timeout should be reasonable (1 hour — delivery is near-instant,
        // the cap only catches a jammed host channel).
        assert_eq!(tool.timeout_seconds(), 3600);
    }

    /// `DelegateToTool` and `SendMessageTool` should differ in sync
    /// semantics: delegate is background (sync_seconds=0), send_message
    /// is synchronous (sync_seconds > 0). This distinction is what makes
    /// them useful for different coordination patterns.
    #[test]
    fn send_message_sync_vs_delegate_background() {
        let (cmd_tx, _cmd_rx) = tokio::sync::mpsc::unbounded_channel::<HostCommand>();
        let (event_tx, _) = tokio::sync::broadcast::channel::<HostEvent>(1);
        let control = HostControl::new(cmd_tx, event_tx);

        let delegate = DelegateToTool {
            control: control.clone(),
        };
        let sender = SendMessageTool { control };

        // delegate_to is background (sync_seconds = 0).
        assert_eq!(delegate.sync_seconds(), 0);
        // send_message is synchronous.
        assert!(sender.sync_seconds() > 0);
    }
}
