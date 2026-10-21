//! Profile for agents that develop RSI plugins.
//!
//! The production prompt lives in the runtime host
//! (`runtime/src/config.rs`); this profile only binds a stable agent
//! identity to the specialized plugin development toolset.

use agentik_core::tools::ToolRegistration;

use crate::{Error, Result, plugin_development_tool_registrations};

/// Configuration for one plugin development agent.
///
/// The identity determines the VFS principal; plugins are selected per
/// tool call by their `/plugins/dev/<name>` path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentProfile {
    agent_id: String,
}

impl AgentProfile {
    /// Create the default profile for an agent identity.
    ///
    /// `agent_id` determines the deterministic VFS principal used by this
    /// profile's tools; it does not need to equal the running agent's UUID.
    pub fn new(agent_id: impl Into<String>) -> Result<Self> {
        let agent_id = agent_id.into();
        validate_agent_id(&agent_id)?;
        Ok(Self { agent_id })
    }

    /// Return the identity used to derive this profile's VFS principal.
    pub fn agent_id(&self) -> &str {
        &self.agent_id
    }

    /// Build the specialized tools bound to this profile's agent identity.
    pub fn tool_registrations(&self) -> Vec<ToolRegistration> {
        plugin_development_tool_registrations(self.agent_id.clone())
    }
}

fn validate_agent_id(agent_id: &str) -> Result<()> {
    if agent_id.is_empty()
        || agent_id.len() > crate::tools::MAX_AGENT_ID_BYTES
        || agent_id.chars().any(|character| character.is_control())
    {
        return Err(Error::Validation(
            "agent id must be nonempty, bounded, and contain no control characters".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_binds_identity_to_plugin_toolset() {
        let profile = AgentProfile::new("plugin-agent").unwrap();
        let tools = profile.tool_registrations();
        let names = tools
            .iter()
            .map(|tool| tool.definition.name.as_str())
            .collect::<Vec<_>>();

        assert_eq!(profile.agent_id(), "plugin-agent");
        assert_eq!(
            names,
            [
                "plugin_environments_list",
                "plugin_environment_bind",
                "plugin_fork",
                "plugin_install",
                "engine_inspect_cache",
                "plugin_uninstall",
                "plugin_node_create",
                "plugin_node_update",
                "plugin_node_update_doc",
                "plugin_container_run",
            ]
        );
    }

    #[test]
    fn profile_rejects_invalid_identities() {
        assert!(AgentProfile::new("invalid\tid").is_err());
    }
}
