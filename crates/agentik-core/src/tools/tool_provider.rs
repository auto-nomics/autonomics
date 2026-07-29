//! Global tool registry: maps tool names to `ToolRegistration`s.
//!
//! Used by `AgentBlueprint` to build a `Toolset` from a skill tree's
//! `allowed_tools` union. Callers register all available tool implementations
//! once; the skill tree determines which subset is active.

use std::collections::HashMap;

use super::{ToolRegistration, Toolset};

/// A registry that maps tool names to their `ToolRegistration`.
///
/// Used to build a `Toolset` dynamically based on which tools
/// a skill tree declares in its `allowed_tools` fields.
pub struct ToolProviderRegistry {
    tools: HashMap<String, ToolRegistration>,
}

impl Default for ToolProviderRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl ToolProviderRegistry {
    /// Create an empty tool provider registry.
    pub fn new() -> Self {
        Self {
            tools: HashMap::new(),
        }
    }

    /// Register a tool. The tool is indexed by `definition.name`.
    ///
    /// Returns the old registration if a tool with the same name was already registered.
    pub fn register(&mut self, reg: ToolRegistration) -> Option<ToolRegistration> {
        let name = reg.definition.name.clone();
        self.tools.insert(name, reg)
    }

    /// Look up a tool registration by name.
    pub fn get(&self, name: &str) -> Option<&ToolRegistration> {
        self.tools.get(name)
    }

    /// Return all registered tool names.
    pub fn names(&self) -> Vec<String> {
        let mut names: Vec<String> = self.tools.keys().cloned().collect();
        names.sort();
        names
    }

    /// Build a `Toolset` containing only the named tools.
    ///
    /// Tools not found in the registry are silently skipped (with a warning log).
    pub fn build_toolset(&self, names: &[String]) -> Toolset {
        let mut toolset = Toolset::new(Some(
            tokio::sync::mpsc::unbounded_channel::<agentik_sdk::types::AgentEvent>().0,
        ));

        for name in names {
            match self.tools.get(name) {
                Some(reg) => {
                    if let Err(e) = toolset.register(reg.clone()) {
                        tracing::warn!(
                            tool_name = %name,
                            error = %e,
                            "failed to register tool from provider"
                        );
                    }
                }
                None => {
                    tracing::warn!(
                        tool_name = %name,
                        "tool declared in skill tree but not found in ToolProviderRegistry"
                    );
                }
            }
        }

        toolset
    }

    /// Return the number of registered tools.
    pub fn len(&self) -> usize {
        self.tools.len()
    }

    /// Return true if no tools are registered.
    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentik_sdk::types::tools::{ToolResult, ToolResultContent};
    use async_trait::async_trait;

    use agentik_proc::tool;

    #[tool(name = "dummy_tool", description = "A dummy tool for testing")]
    struct DummyInput {
        #[desc = "reason"]
        pub reason: String,
    }

    struct DummyTool;

    #[async_trait]
    impl super::super::function::ToolFunction for DummyTool {
        type Input = DummyInput;

        async fn run(
            &self,
            input: DummyInput,
        ) -> Result<ToolResult, super::super::error::ToolError> {
            Ok(ToolResult {
                tool_use_id: "dummy".to_string(),
                content: ToolResultContent::Text(format!("dummy: {}", input.reason)),
                is_error: None,
            })
        }
    }

    fn make_reg() -> ToolRegistration {
        ToolRegistration::from(DummyTool)
    }

    #[test]
    fn test_register_and_get() {
        let mut reg = ToolProviderRegistry::new();
        reg.register(make_reg());

        assert!(reg.get("dummy_tool").is_some());
        assert!(reg.get("nonexistent").is_none());
        assert_eq!(reg.len(), 1);
    }

    #[test]
    fn test_names() {
        let mut reg = ToolProviderRegistry::new();
        reg.register(make_reg());

        let names = reg.names();
        assert!(names.contains(&"dummy_tool".to_string()));
    }

    #[tokio::test]
    async fn test_build_toolset() {
        let mut reg = ToolProviderRegistry::new();
        reg.register(make_reg());

        let toolset = reg.build_toolset(&["dummy_tool".to_string()]);
        let tools = toolset.tools();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, "dummy_tool");
    }

    #[tokio::test]
    async fn test_build_toolset_skips_missing() {
        let reg = ToolProviderRegistry::new();
        let toolset = reg.build_toolset(&["nonexistent".to_string()]);
        assert_eq!(toolset.tools().len(), 0);
    }
}
