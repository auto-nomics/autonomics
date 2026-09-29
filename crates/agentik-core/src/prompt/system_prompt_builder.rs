/// Layers (in order)
/// 1. Agent identity: role + the agent's own name (for multi-agent interactions).
/// 2. SOP: specify the usages, examples of available tools (when to use, how to use).
///    Does NOT include schema of tools (passed directly to LlmClient).
#[derive(Default)]
pub struct SystemPromptBuilder {
    identity: String,
    tooluse_guidance: String,
    extra_sections: Vec<String>,
}
impl SystemPromptBuilder {
    pub fn with_identity(mut self, identity: impl Into<String>) -> Self {
        self.identity = identity.into();
        self
    }

    /// Append an extra section. Sections render in insertion order, so a
    /// later section (e.g. memory) supplements — never replaces — an
    /// earlier one (e.g. the agent's profile prompt).
    pub fn with_extra_section(mut self, section: impl Into<String>) -> Self {
        self.extra_sections.push(section.into());
        self
    }

    pub fn with_tooluse_guidance(mut self, guidance: impl Into<String>) -> Self {
        self.tooluse_guidance = guidance.into();
        self
    }

    pub fn build_tooluse_guidance(mut self) -> Self {
        self.tooluse_guidance = concat!(
            "## Tool usage\n",
            "Use tools to complete the task. When operations are independent, you should return multiple tool calls in a single response. For example, when creating multiple entities or linking several isolated knowledge entries, issue all tool calls together in one reply rather than one at a time.\n",
            "Tool calls within a single reply execute in parallel, which greatly reduces round-trip time.\n\n",
            "## Task completion\n",
            "When all tasks are complete, output your final text directly — a response with no tool calls signals task completion. No additional termination action is needed.\n\n",
            include_str!("plan_guidance.md"),
        ).to_string();
        self
    }

    pub fn parse(self) -> String {
        let mut system_prompt = String::new();

        if !self.identity.is_empty() {
            system_prompt.push_str(&self.identity);
            system_prompt.push('\n');
        }
        for section in &self.extra_sections {
            system_prompt.push_str(section);
            system_prompt.push('\n');
        }
        if !self.tooluse_guidance.is_empty() {
            system_prompt.push_str(&self.tooluse_guidance);
            system_prompt.push('\n');
        }

        system_prompt
    }
}
