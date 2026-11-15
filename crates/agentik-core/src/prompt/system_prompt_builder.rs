/// Layers (in order)
/// 1. Agent identity: role + the agent's own name (for multi-agent interactions).
/// 2. SOP: specify the usages, examples of available tools (when to use, how to use).
///    Does NOT include schema of tools (passed directly to LlmClient).
#[derive(Default)]
pub struct SystemPromptBuilder {
    identity: String,
    tooluse_guidance: String,
    wiki_guidance: String,
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
            include_str!("system_prompt/tooluse_guidance.md"),
            "\n",
            include_str!("system_prompt/plan_guidance.md"),
            "\n",
            include_str!("system_prompt/intelligent_ui_guidance.md"),
            "\n",
            include_str!("system_prompt/resource_links_guidance.md"),
        )
        .to_string();
        self
    }

    /// Append the wiki knowledge-base guidance section: when to distill
    /// conversation/literature findings into the wiki and the maintenance
    /// rules. Static behavioral guidance, so it belongs in the system prompt.
    pub fn build_wiki_guidance(mut self) -> Self {
        self.wiki_guidance =
            concat!("\n", include_str!("system_prompt/wiki_guidance.md")).to_string();
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
        if !self.wiki_guidance.is_empty() {
            system_prompt.push_str(&self.wiki_guidance);
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
    fn intelligent_ui_guidance_is_included_in_standard_guidance() {
        let prompt = SystemPromptBuilder::default()
            .build_tooluse_guidance()
            .parse();
        assert!(prompt.contains("## Intelligent UI"));
        assert!(prompt.contains("```generative-ui"));
        assert!(prompt.contains("jsExpressions"));
        assert!(prompt.contains("CopilotKit"));
        assert!(
            !SystemPromptBuilder::default()
                .parse()
                .contains("## Intelligent UI")
        );
    }

    #[test]
    fn wiki_guidance_renders_only_when_built() {
        let plain = SystemPromptBuilder::default()
            .with_identity("id")
            .build_tooluse_guidance()
            .parse();
        assert!(!plain.contains("Wiki knowledge base"));

        let with_wiki = SystemPromptBuilder::default()
            .with_identity("id")
            .build_wiki_guidance()
            .build_tooluse_guidance()
            .parse();
        assert!(with_wiki.contains("## Wiki knowledge base"));
        assert!(with_wiki.contains("### Skills are ordinary wiki entries"));
        assert!(!with_wiki.contains("evo_observe"));
        // Render order: identity → extras → wiki guidance → tool-use
        // guidance, mirroring parse().
        let wiki_at = with_wiki.find("## Wiki knowledge base").unwrap();
        let tool_at = with_wiki.find("## Tool usage").unwrap();
        assert!(
            wiki_at < tool_at,
            "wiki guidance renders before tool-use guidance"
        );
    }
}
