//! Agent tools for controlling the multi-agent host.
//!
//! Each tool wraps a [`HostControl`] command and exposes it to the LLM as
//! a callable function. Agents use these tools to spawn peers, manage
//! topology, send messages, and query system status.

use agentik_core::tools::{ToolContext, ToolFunction, ToolRegistration};
use agentik_network::TerminationSpec;
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
            caller_path: self_path.as_str().to_string(),
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
        ToolRegistration::from(ListDelegationsTool {
            control: ctrl.clone(),
            caller_path: self_path.as_str().to_string(),
        }),
        ToolRegistration::from(GetAgentHistoryTool {
            control: ctrl.clone(),
        }),
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
        match self
            .control
            .spawn_agent(
                &input.agent_name,
                &self.caller_path,
                &self.caller_profile_path,
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
    enable_chembl: Option<bool>,
    #[desc = "Enable RCSB PDB search, summaries, polymer entities, and structure previews."]
    enable_rcsb: Option<bool>,
    #[desc = "Enable STRING identifier resolution, interactions, enrichment, summaries, and previews."]
    enable_string: Option<bool>,
    #[desc = "Enable KEGG metadata, search, entry previews, biological links, ID mapping, and DDI queries."]
    enable_kegg: Option<bool>,
    enable_dag_history: Option<bool>,
    /// Enable or disable memory injection, tools, and generation together.
    use_memory: Option<bool>,
    /// Override memory generation independently.
    generate_memory: Option<bool>,
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
            enable_chembl: input.enable_chembl,
            enable_rcsb: input.enable_rcsb,
            enable_string: input.enable_string,
            enable_kegg: input.enable_kegg,
            enable_dag_history: input.enable_dag_history,
            use_memory: input.use_memory,
            generate_memory: input.generate_memory.or(input.use_memory),
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
            Err(e) => Ok(ToolResult::success(format!("Derive failed: {e}"))),
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Delegate To Agent (request-response, background async)
// ═══════════════════════════════════════════════════════════════════════

#[tool(
    name = "delegate_to",
    description = "Delegate a task to another agent. Runs in the background and returns \
                   a task number (#N) immediately. Use `wait_task` with the task number \
                   to block until the result is ready, or `view_task_results` to poll \
                   for the output. Multiple delegates can run concurrently. \
                   The target agent's COMPLETE response becomes the task's result."
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
        match self.control.delegate(&input.agent_name, input.task).await {
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
// Send Message — inter-agent notification without response injection (Phase 5)
// ═══════════════════════════════════════════════════════════════════════

#[tool(
    name = "send_message",
    description = "Send a message to another agent WITHOUT receiving its response. \
                   Unlike delegate_to, the target agent's output is NOT injected back \
                   into your context — you only get a delivery confirmation. Use this when:\n\
                   - You want to notify another agent without needing its reply.\n\
                   - You want to kick off work and check results later via list_agents.\n\
                   - You need to broadcast to multiple agents without consuming each response.\n\
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
    /// confirms delivery (name resolution + channel send). The key
    /// Sync (default) — delivery confirmation returned immediately.
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
            Some(Err(e)) => Ok(ToolResult::success(format!("send_message failed: {e}"))),
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
    description = "Read an agent's persisted conversation history. Returns \
                   the most recent messages for the agent, which can be used \
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
            Some(history) => Ok(ToolResult::success_json(
                serde_json::to_value(history).unwrap_or_default(),
            )),
            None => Ok(ToolResult::success(
                "Failed to read agent history — host unavailable.",
            )),
        }
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

fn parse_termination(s: &str) -> crate::error::Result<agentik_network::TerminationSpec> {
    let (kind, rest) = s
        .split_once(':')
        .ok_or_else(|| crate::error::Error::Other(format!("expected 'kind:args', got '{s}'")))?;
    match kind {
        "max_rounds" => {
            let max: usize = rest
                .parse()
                .map_err(|_| crate::error::Error::Other("max_rounds needs a number".into()))?;
            Ok(TerminationSpec::MaxRounds { max })
        }
        "condition" => {
            let (node, pattern) = rest.split_once(':').ok_or_else(|| {
                crate::error::Error::Other("condition needs 'node:pattern'".into())
            })?;
            Ok(TerminationSpec::Condition {
                node: node.into(),
                pattern: pattern.into(),
            })
        }
        "any_node_done" => {
            let nodes: Vec<String> = rest.split(',').map(|s| s.trim().to_string()).collect();
            Ok(TerminationSpec::AnyNodeDone { nodes })
        }
        other => Err(crate::error::Error::Other(format!(
            "unknown termination kind: '{other}'"
        ))),
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
    /// `view_task_results` to see the Cancelled status.
    /// Sync (default) — fast fire-and-forget, returns near-instantly.
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
    use agentik_core::tools::{ExecutionMode, ToolFunction};

    /// The `SendMessageTool` should be Sync — the calling agent gets
    /// immediate delivery confirmation.
    #[test]
    fn send_message_tool_is_sync() {
        let (cmd_tx, _cmd_rx) = tokio::sync::mpsc::unbounded_channel::<HostCommand>();
        let (event_tx, _) = tokio::sync::broadcast::channel::<HostEvent>(1);
        let control = HostControl::new(cmd_tx, event_tx);
        let tool = SendMessageTool { control };

        assert_eq!(tool.execution_mode(), ExecutionMode::Sync);
        assert_eq!(tool.timeout_seconds(), 3600);
    }

    /// `DelegateToTool` is Async (result pulled on demand after notification).
    /// `SendMessageTool` is Sync (no response injection). This is the
    /// key semantic distinction.
    #[test]
    fn send_message_sync_vs_delegate_async() {
        let (cmd_tx, _cmd_rx) = tokio::sync::mpsc::unbounded_channel::<HostCommand>();
        let (event_tx, _) = tokio::sync::broadcast::channel::<HostEvent>(1);
        let control = HostControl::new(cmd_tx, event_tx);

        let delegate = DelegateToTool {
            control: control.clone(),
            caller_path: "/root/caller".into(),
        };
        let sender = SendMessageTool { control };

        assert_eq!(delegate.execution_mode(), ExecutionMode::Async);
        assert_eq!(sender.execution_mode(), ExecutionMode::Sync);
    }

    #[tokio::test]
    async fn concurrent_delegation_commands_have_distinct_ids() {
        let (cmd_tx, mut cmd_rx) = tokio::sync::mpsc::unbounded_channel::<HostCommand>();
        let (event_tx, _) = tokio::sync::broadcast::channel::<HostEvent>(1);
        let control = HostControl::new(cmd_tx, event_tx);
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
