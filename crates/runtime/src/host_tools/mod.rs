//! Agent tools for controlling the multi-agent host.
//!
//! Each tool wraps a [`HostControl`] command and exposes it to the LLM as
//! a callable function. Agents use these tools to spawn their own child
//! agents, delegate tasks to them, and manage/query the agents visible to
//! them. Inter-agent communication is delegation-only and strictly
//! parent→direct-child; every tool below enforces that one-hop scope.

use agentik_core::tools::ToolRegistration;

use crate::control::HostControl;

mod delegate_to;
mod get_agent_history;
mod get_agent_info;
mod interrupt_agent;
mod list_agents;
mod list_delegations;
mod spawn_agent;

use delegate_to::DelegateToTool;
use get_agent_history::GetAgentHistoryTool;
use get_agent_info::GetAgentInfoTool;
use interrupt_agent::InterruptAgentTool;
use list_agents::ListAgentsTool;
use list_delegations::ListDelegationsTool;
use spawn_agent::SpawnAgentTool;

/// One-hop visibility: an agent sees itself, its direct parent, and its
/// direct children — nothing else.
fn visible_from(caller: &agentik_types::AgentPath, other: &agentik_types::AgentPath) -> bool {
    *other == *caller || other.is_direct_child_of(caller) || caller.is_direct_child_of(other)
}

/// Build the full set of host control tools for an agent.
/// Returns an empty vec if `control` is `None`.
///
/// `self_path` is the calling agent's own hierarchical path. It is used to:
/// - Scope `list_agents` / `get_agent_info` / `get_agent_history` results
///   to the agents visible to the caller (self,
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
        ToolRegistration::from(InterruptAgentTool {
            control: ctrl.clone(),
            self_path: self_path.clone(),
        }),
    ]
}

#[cfg(test)]
mod tests;
