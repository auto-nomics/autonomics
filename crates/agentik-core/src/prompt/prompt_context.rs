use agentik_sdk::types::Message;

use agentik_sdk::AgentMessageExt;

/// Builder for a prompt request: optional system prompt plus conversation
/// messages. (Renamed from `prompt::Context` to free the `Context` name for
/// the agent's working context type.)
#[derive(Debug, Default)]
pub struct PromptContext {
    system_prompt: Option<String>,
    conversations: Vec<Message>,
}

impl PromptContext {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_system_prompt(mut self, system_prompt: String) -> Self {
        self.system_prompt = Some(system_prompt);
        self
    }

    pub fn with_conversations(mut self, conversations: Vec<Message>) -> Self {
        self.conversations = conversations;
        self
    }

    pub fn build(self) -> Vec<Message> {
        let mut messages = Vec::with_capacity(1 + self.conversations.len());

        if let Some(system_prompt) = self.system_prompt {
            messages.push(Message::system(system_prompt));
        }

        messages.extend(self.conversations);
        messages
    }
}
