/// Layers (in order)
/// 1. Agent identity: role + the agent's own name (for multi-agent interactions).
/// 2. SOP: specify the usages, examples of available tools (when to use, how to use).
///    Does NOT include schema of tools (passed directly to LlmClient).
#[derive(Default)]
pub struct SystemPromptBuilder {
    identity: String,
    tooluse_guidance: String,
    skill_guidance: String,
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

    /// Append the tool-use guidance section: parallel tool calls,
    /// task completion signaling, and plan-mode usage. Static
    /// behavioral guidance, so it belongs in the system prompt.
    pub fn build_tooluse_guidance(mut self) -> Self {
        self.tooluse_guidance = concat!(
            include_str!("tooluse_guidance.md"),
            "\n",
            include_str!("plan_guidance.md"),
        )
        .to_string();
        self
    }

    /// Append the skill-library guidance section: how to consume
    /// skills (search/get/workflows), how to feed the evolution loop
    /// (observe/propose/evolve), and the judgement boundaries. Static
    /// behavioral guidance, so it belongs in the system prompt (the
    /// per-session skill *index* is injected separately by the runtime
    /// and stays snapshot-stable for prompt-cache friendliness).
    pub fn build_skill_guidance(mut self) -> Self {
        self.skill_guidance = concat!("\n", include_str!("skill_guidance.md"),).to_string();
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
        if !self.skill_guidance.is_empty() {
            system_prompt.push_str(&self.skill_guidance);
            system_prompt.push('\n');
        }
        if !self.tooluse_guidance.is_empty() {
            system_prompt.push_str(&self.tooluse_guidance);
            system_prompt.push('\n');
        }

        system_prompt
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skill_guidance_renders_only_when_built() {
        let plain = SystemPromptBuilder::default()
            .with_identity("id")
            .build_tooluse_guidance()
            .parse();
        assert!(!plain.contains("Skill library"));

        let with_skills = SystemPromptBuilder::default()
            .with_identity("id")
            .build_skill_guidance()
            .build_tooluse_guidance()
            .parse();
        assert!(with_skills.contains("## Skill library"));
        assert!(with_skills.contains("skill_observe"));
        assert!(with_skills.contains("skill_propose"));
        // Render order: identity → extras → skill guidance → tool-use
        // guidance, mirroring parse().
        let skill_at = with_skills.find("## Skill library").unwrap();
        let tool_at = with_skills.find("## Tool usage").unwrap();
        assert!(
            skill_at < tool_at,
            "skill guidance renders before tool-use guidance"
        );
    }
}
