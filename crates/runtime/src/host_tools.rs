//! Agent tools for controlling the multi-agent host.
//!
//! Each tool wraps a [`HostControl`] command and exposes it to the LLM as
//! a callable function. Agents use these tools to spawn peers, manage
//! topology, send messages, and query system status.

use agentik_core::tools::{ToolFunction, ToolRegistration};
use agentik_network::{EdgeTrigger, TerminationSpec};
use agentik_proc::tool;
use agentik_sdk::types::ToolResult;
use async_trait::async_trait;

use crate::control::HostControl;

/// Build the full set of host control tools for an agent.
/// Returns an empty vec if `control` is `None`.
pub fn host_tools(control: Option<HostControl>) -> Vec<ToolRegistration> {
    let Some(ctrl) = control else {
        return vec![];
    };
    vec![
        ToolRegistration::from(SpawnAgentTool { control: ctrl.clone() }),
        ToolRegistration::from(SendToAgentTool { control: ctrl.clone() }),
        ToolRegistration::from(DelegateToTool { control: ctrl.clone() }),
        ToolRegistration::from(ListAgentsTool { control: ctrl.clone() }),
        ToolRegistration::from(ConnectAgentsTool { control: ctrl.clone() }),
        ToolRegistration::from(DisconnectAgentsTool { control: ctrl.clone() }),
        ToolRegistration::from(SetTerminationTool { control: ctrl.clone() }),
        ToolRegistration::from(GetNetworkStatusTool { control: ctrl.clone() }),
        ToolRegistration::from(ShutdownAgentTool { control: ctrl.clone() }),
        ToolRegistration::from(ResetNetworkTool { control: ctrl.clone() }),
        ToolRegistration::from(InjectPromptsTool { control: ctrl }),
    ]
}

// ═══════════════════════════════════════════════════════════════════════
// Spawn Agent
// ═══════════════════════════════════════════════════════════════════════

#[tool(
    name = "spawn_agent",
    description = "Spawn a new agent and register it with the host. \
                   The agent will be created from an existing profile name \
                   and immediately available for message routing."
)]
struct SpawnAgentInput {
    /// Unique name for the new agent instance.
    agent_name: String,
    /// Name of the AgentProfile to instantiate (must already exist).
    profile_name: String,
}

struct SpawnAgentTool {
    control: HostControl,
}

#[async_trait]
impl ToolFunction for SpawnAgentTool {
    type Input = SpawnAgentInput;

    async fn run(&self, input: SpawnAgentInput) -> Result<ToolResult, agentik_core::tools::ToolError> {
        match self
            .control
            .spawn_agent(&input.agent_name, &input.profile_name)
            .await
        {
            Ok(name) => Ok(ToolResult::success(format!(
                "Agent '{name}' spawned and registered."
            ))),
            Err(e) => Ok(ToolResult::success(format!("Spawn failed: {e}"))),
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Send To Agent
// ═══════════════════════════════════════════════════════════════════════

#[tool(
    name = "send_to_agent",
    description = "Send a text message to a named agent. \
                   The message appears as a user-style input to the target agent."
)]
struct SendToAgentInput {
    /// Name of the target agent.
    agent_name: String,
    /// Message text to deliver.
    message: String,
}

struct SendToAgentTool {
    control: HostControl,
}

#[async_trait]
impl ToolFunction for SendToAgentTool {
    type Input = SendToAgentInput;

    async fn run(&self, input: SendToAgentInput) -> Result<ToolResult, agentik_core::tools::ToolError> {
        self.control.send_to(&input.agent_name, input.message);
        Ok(ToolResult::success(format!(
            "Message delivered to '{}'.",
            input.agent_name
        )))
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
    /// Name of the target agent to delegate to.
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

    async fn run(&self, input: DelegateToInput) -> Result<ToolResult, agentik_core::tools::ToolError> {
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
// List Agents / Network Status
// ═══════════════════════════════════════════════════════════════════════

#[tool(
    name = "list_agents",
    description = "List all registered agents and current network topology status as JSON."
)]
struct ListAgentsInput {}

struct ListAgentsTool {
    control: HostControl,
}

#[async_trait]
impl ToolFunction for ListAgentsTool {
    type Input = ListAgentsInput;

    async fn run(&self, _input: ListAgentsInput) -> Result<ToolResult, agentik_core::tools::ToolError> {
        match self.control.get_status().await {
            Some(status) => Ok(ToolResult::success_json(
                serde_json::to_value(&status).unwrap_or_default(),
            )),
            None => Ok(ToolResult::success("Failed to get host status.")),
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Connect Agents
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

    async fn run(&self, input: ConnectAgentsInput) -> Result<ToolResult, agentik_core::tools::ToolError> {
        let trigger = match input.trigger.as_str() {
            "on_done" | "done" => EdgeTrigger::OnDone,
            "on_pattern" | "pattern" => EdgeTrigger::OnPattern {
                pattern: input.pattern.unwrap_or_default(),
            },
            other => {
                return Ok(ToolResult::success(format!(
                    "Unknown trigger '{other}'. Use 'on_done' or 'on_pattern'."
                )))
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

    async fn run(&self, input: DisconnectAgentsInput) -> Result<ToolResult, agentik_core::tools::ToolError> {
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

    async fn run(&self, input: SetTerminationInput) -> Result<ToolResult, agentik_core::tools::ToolError> {
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

    async fn run(&self, _input: GetNetworkStatusInput) -> Result<ToolResult, agentik_core::tools::ToolError> {
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
    description = "Shut down a named agent and remove it from the registry."
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

    async fn run(&self, input: ShutdownAgentInput) -> Result<ToolResult, agentik_core::tools::ToolError> {
        self.control.shutdown_agent(&input.agent_name);
        Ok(ToolResult::success(format!(
            "Agent '{}' shutdown requested.",
            input.agent_name
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

    async fn run(&self, _input: ResetNetworkInput) -> Result<ToolResult, agentik_core::tools::ToolError> {
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

    async fn run(&self, _input: InjectPromptsInput) -> Result<ToolResult, agentik_core::tools::ToolError> {
        self.control.inject_prompts();
        Ok(ToolResult::success("Initial prompts injected."))
    }
}
