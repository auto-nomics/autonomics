//! Profile for agents that develop RSI plugins.

use agentik_core::agent_builder::AgentBuilder;
use agentik_core::tools::ToolRegistration;

use crate::{
    Error, PluginDevelopment, PluginDevelopmentToolsetRegistry, Result,
    plugin_development_tool_registrations, tools::PluginDevelopmentBinding,
};

/// Configuration for one plugin development agent.
///
/// The profile connects one stable agent identity to the specialized plugin
/// development toolset. It does not own runtime state: candidate leases remain
/// in [`PluginDevelopmentToolsetRegistry`], while the host supplies the model
/// and lifecycle when it builds the agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentProfile {
    agent_id: String,
    agent_identity: String,
    system_prompt: String,
}

impl AgentProfile {
    /// Create the default profile for an agent identity.
    ///
    /// `agent_id` is the binding identity used by the process-wide toolset
    /// registry; it does not need to equal the UUID of the running agent.
    pub fn new(agent_id: impl Into<String>) -> Result<Self> {
        let agent_id = agent_id.into();
        validate_agent_id(&agent_id)?;
        Ok(Self {
            agent_id,
            agent_identity: "You are a plugin development agent.".into(),
            system_prompt: default_system_prompt(),
        })
    }

    /// Return the identity used to bind this profile to a plugin.
    pub fn agent_id(&self) -> &str {
        &self.agent_id
    }

    /// Return the identity shown in the agent system prompt.
    pub fn agent_identity(&self) -> &str {
        &self.agent_identity
    }

    /// Replace the identity shown in the agent system prompt.
    pub fn set_agent_identity(&mut self, identity: impl Into<String>) -> Result<()> {
        let identity = identity.into();
        if identity.trim().is_empty() {
            return Err(Error::Validation("agent identity cannot be empty".into()));
        }
        self.agent_identity = identity;
        Ok(())
    }

    /// Return the specialized system prompt section for plugin development.
    pub fn system_prompt(&self) -> &str {
        &self.system_prompt
    }

    /// Replace the specialized system prompt section for plugin development.
    pub fn set_system_prompt(&mut self, prompt: impl Into<String>) -> Result<()> {
        let prompt = prompt.into();
        if prompt.trim().is_empty() {
            return Err(Error::Validation(
                "agent system prompt cannot be empty".into(),
            ));
        }
        self.system_prompt = prompt;
        Ok(())
    }

    /// Build the specialized tools bound to this profile's agent identity.
    pub fn tool_registrations(&self) -> Vec<ToolRegistration> {
        plugin_development_tool_registrations(self.agent_id.clone())
    }

    /// Bind this agent identity to one plugin development candidate.
    pub fn bind_plugin(
        &self,
        development: &mut PluginDevelopment<'_>,
        run_id: &str,
    ) -> Result<PluginDevelopmentBinding> {
        PluginDevelopmentToolsetRegistry::global().bind_plugin_agent(
            self.agent_id.as_str(),
            development,
            run_id,
        )
    }

    /// Attach the profile's prompts and specialized tools to an agent builder.
    ///
    /// The caller remains responsible for supplying the model, storage,
    /// memory, path, and other lifecycle configuration.
    pub fn apply_to_agent_builder(&self, builder: AgentBuilder) -> AgentBuilder {
        builder
            .with_tools(self.tool_registrations())
            .with_system_prompt_identity(self.agent_identity.clone())
            .with_system_prompt_section(self.system_prompt.clone())
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

fn default_system_prompt() -> String {
    "You develop exactly one assigned plugin. Start with plugin_development_status, \
    modify its nodes and files only through the plugin tools, and use \
    plugin_container_run to turn failures into implementation feedback. Do not claim \
    completion without checking the candidate in its selected environment."
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_connects_agent_identity_to_plugin_toolset() {
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
                "plugin_development_status",
                "plugin_node_spec",
                "plugin_node_create",
                "plugin_node_update_doc",
                "plugin_node_read_script",
                "plugin_node_write_script",
                "plugin_workspace_list",
                "plugin_workspace_read",
                "plugin_workspace_write",
                "plugin_container_run",
            ]
        );
        assert!(
            profile
                .system_prompt()
                .contains("exactly one assigned plugin")
        );
    }

    #[test]
    fn profile_validates_identity_fields() {
        let mut profile = AgentProfile::new("plugin-agent").unwrap();
        assert!(profile.set_agent_identity("  ").is_err());
        assert!(profile.set_system_prompt(String::new()).is_err());
        assert!(AgentProfile::new("invalid\tid").is_err());
    }
}
