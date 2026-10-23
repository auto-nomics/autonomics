use agentik_sdk::types::Message;

use agentik_sdk::AgentMessageExt;

#[derive(Debug, Default)]
pub struct Context {
    system_prompt: Option<String>,
    conversations: Vec<Message>,
}

impl Context {
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
