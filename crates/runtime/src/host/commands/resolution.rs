//! Name resolution and relay command forwarding: mapping agent
//! references (full path or short name) to registry keys, from both the
//! operator's global viewpoint and a caller's scoped viewpoint.

use super::super::relay::AgentCommand;
use super::super::RuntimeHost;

impl RuntimeHost {
    /// Resolve an agent reference to its full path key in the registry.
    ///
    /// Accepts:
    /// 1. **Full path** (`/root/researcher/worker`) — exact match.
    /// 2. **Short name** (`researcher`) — matches when unambiguous.
    ///
    /// Returns the HashMap key string, or `None` if no match / ambiguous.
    pub(super) fn resolve_agent(&self, target: &str) -> Option<String> {
        // 1. Exact full-path match.
        if target.starts_with("/root") && self.agents.contains_key(target) {
            return Some(target.to_string());
        }
        // 2. Short-name match: collect all agents whose last path segment
        //    equals `target`.
        let matches: Vec<&String> = self
            .agents
            .keys()
            .filter(|key| key.rsplit('/').next().unwrap_or(key) == target)
            .collect();
        match matches.len() {
            1 => Some(matches[0].clone()),
            0 => None,
            _ => {
                tracing::warn!(
                    target = %target,
                    candidates = ?matches,
                    "ambiguous agent name — use full path to disambiguate"
                );
                None
            }
        }
    }

    /// Resolve a path and require the corresponding live agent registry entry.
    pub(super) fn resolve_live_agent(
        &self,
        path: &str,
        role: &str,
        requested: &str,
    ) -> std::result::Result<String, String> {
        let resolved = self
            .resolve_agent(path)
            .unwrap_or_else(|| requested.to_string());
        if !self.agents.contains_key(&resolved) {
            return Err(format!(
                "Agent '{requested}' resolved to '{resolved}' but is not currently running. \
                 Only live agents can be the {role} of an inter-agent operation."
            ));
        }
        Ok(resolved)
    }

    /// Resolve a delegation target from the caller's viewpoint: full paths
    /// must match exactly; short names prefer the caller's direct child
    /// before falling back to the global scan (the delegation validator
    /// still gates whatever comes back).
    pub(super) fn resolve_agent_scoped(
        &self,
        caller: &agentik_types::AgentPath,
        to: &str,
    ) -> Option<String> {
        // Virtual-actor: liveness is no longer a resolution requirement.
        // Full paths are accepted verbatim and short names resolve to the
        // caller's child path — a dormant identity activates on delivery,
        // and a nonexistent one fails activation with a clear reason.
        if to.starts_with("/root") {
            return Some(to.to_string());
        }
        if let Ok(child) = caller.join(to) {
            return Some(child.as_str().to_string());
        }
        self.resolve_agent(to)
    }

    /// Forward a command to a named agent's relay task.
    /// `name` is resolved via [`resolve_agent`](Self::resolve_agent).
    pub(super) fn send_agent_command(&self, name: &str, cmd: AgentCommand) {
        if let Some(resolved) = self.resolve_agent(name) {
            if let Some(entry) = self.agents.get(&resolved) {
                let _ = entry.cmd_tx.send(cmd);
                return;
            }
        }
        tracing::warn!(agent = %name, "send_agent_command: agent not registered");
    }
}

/// Uniform denial for agent-initiated management of a target that is not a
/// resolvable direct child. Deliberately identical for "unresolvable" and
/// "exists but not your child" so callers cannot probe the registry.
pub(super) fn not_a_direct_child_message(name: &str) -> String {
    format!(
        "Agent '{name}' is not one of your direct children. \
         Agents may only manage their own direct children; \
         use list_agents to see them."
    )
}

/// True when `ancestor` lies on `path`'s parent chain.
pub(super) fn path_is_ancestor(
    ancestor: &agentik_types::AgentPath,
    path: &agentik_types::AgentPath,
) -> bool {
    let mut current = path.parent();
    while let Some(parent) = current {
        if &parent == ancestor {
            return true;
        }
        current = parent.parent();
    }
    false
}
